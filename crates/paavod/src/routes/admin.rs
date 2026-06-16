//! /admin/* handlers.
//!
//! Dev-loop reset endpoints. The whole module is gated on the operator
//! genuinely wanting a destructive operation — `paavo-cli admin purge`
//! is the only documented client. v1 has no auth, no audit log, no
//! soft-delete; production operators with valuable history should
//! rely on retention sweeps (§8) instead.

use crate::app_state::AppState;
use crate::state_dir::StateDir;
use axum::extract::State;
use axum::http::StatusCode;
use std::path::Path;
use tracing::{error, info, warn};

/// Shorthand for handler results.
type HandlerResult<T> = Result<T, (StatusCode, String)>;

/// `POST /admin/purge` — see §9.5.
///
/// Wipes everything under `${state_dir}/{sandboxes,uploads,cargo-target}`
/// on disk and truncates `job`, `log_frame`, `build_cache` in the DB.
/// Preserves `board` and `schedule` rows (operators do not want to
/// re-register probes after a purge).
///
/// Refuses with `409 Conflict` if any job is currently `building` or
/// `running` — the dispatcher must not have its sandbox dir yanked
/// mid-flash. The operator should `paavo-cli cancel <id>` or wait for
/// in-flight jobs to terminate before purging.
///
/// **Lock ordering:** takes `db.lock()` for the in-flight check + DB
/// truncate in one scope, drops the guard, then performs the
/// best-effort filesystem wipe. DB truncate is the authoritative
/// operation; the filesystem wipe is opportunistic — if a directory
/// is locked (e.g. by an antivirus scanner), we warn and continue.
/// Operators can re-run the purge to retry the disk side.
pub async fn purge(State(s): State<AppState>) -> HandlerResult<StatusCode> {
    // 1. Authoritative DB-side gate + truncate.
    {
        let db = s.db.lock();
        let conn = db.raw_conn();
        let in_flight: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM job WHERE state IN ('building','awaiting_board','running')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| {
                error!(error = %e, "purge: failed to count in-flight jobs");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("count in-flight: {e}"),
                )
            })?;
        if in_flight > 0 {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "purge refused: {in_flight} job(s) currently building, awaiting board, or running; \
                     cancel or wait for them to terminate first"
                ),
            ));
        }
        // log_frame has ON DELETE CASCADE from job, but truncate it
        // explicitly so the rowid sequence resets cleanly and any
        // orphan rows (shouldn't exist; defensive) are also gone.
        for table in ["log_frame", "build_cache", "job"] {
            conn.execute(&format!("DELETE FROM {table}"), [])
                .map_err(|e| {
                    error!(error = %e, table, "purge: DELETE FROM failed");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("DELETE FROM {table}: {e}"),
                    )
                })?;
        }
        info!("purge: db rows cleared (job + log_frame + build_cache)");
    }

    // 2. Best-effort filesystem wipe. The DB is now authoritative —
    //    any orphaned tar / sandbox / cached ELF can never be
    //    referenced by a row, so leaving them around is a disk-leak,
    //    not a correctness problem.
    let sd = StateDir::from_root(&s.config.server.state_dir);
    for dir in [&sd.sandboxes_dir, &sd.uploads_dir, &sd.cargo_target_dir] {
        wipe_dir_contents_lossy(dir);
    }
    // The cache/elf subtree lives under cache/ (see StateDir::from_root).
    // build_cache rows are already gone, so the on-disk ELFs are
    // orphans too — sweep them.
    wipe_dir_contents_lossy(&sd.cache_elfs_dir);

    info!("purge: complete");
    Ok(StatusCode::NO_CONTENT)
}

/// Delete every entry under `dir` (files + subdirs) but leave `dir`
/// itself in place. Failures are logged + ignored; the DB is the
/// source of truth and operators can re-run purge to retry.
fn wipe_dir_contents_lossy(dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Dir doesn't exist (e.g. fresh state dir) — nothing to do.
            return;
        }
        Err(e) => {
            warn!(error = %e, path = %dir.display(), "purge: read_dir failed");
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let result = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = result {
            warn!(error = %e, path = %path.display(), "purge: remove failed");
        }
    }
}
