//! /jobs/* handlers.

use crate::app_state::AppState;
use crate::state_dir::StateDir;
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use paavo_core::{enqueue_job, validate_enqueue, EnqueueRequest};
use paavo_proto::{BoardSelector, JobId, JobSource, Priority};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::{error, warn};

/// JSON metadata part on `POST /jobs`. `source` is NOT here — every
/// HTTP submit is recorded as `JobSource::Cli`; the scheduler reaches
/// `enqueue_job` directly. Unknown fields are rejected with 400 so
/// the wire schema is unambiguous.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostJobMetadata {
    /// Scheduler priority.
    pub priority: Priority,
    /// Free text id; no auth.
    pub submitter: String,
    /// Selector.
    pub board_selector: BoardSelector,
    /// Optional inactivity override (ms). Defaults to
    /// `timeouts.default_inactivity_s * 1000`.
    #[serde(default)]
    pub inactivity_timeout_ms: Option<u64>,
    /// Optional hard-max override (ms). Defaults to
    /// `timeouts.default_ad_hoc_hard_max_s * 1000`.
    #[serde(default)]
    pub hard_max_ms: Option<u64>,
}

/// 202 response body.
#[derive(Debug, Serialize)]
pub struct AcceptedBody {
    /// Newly assigned job id.
    pub job_id: String,
}

type HandlerResult<T> = Result<T, (StatusCode, String)>;

/// POST /jobs.
pub async fn post_jobs(
    State(s): State<AppState>,
    mut multipart: Multipart,
) -> HandlerResult<(StatusCode, Json<AcceptedBody>)> {
    if s.drain.is_draining() {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "paavod is draining".into()));
    }

    let job_id = JobId::new();
    let sd = StateDir::from_root(&s.config.server.state_dir);
    sd.ensure_dirs()
        .map_err(|e| internal("ensure_dirs", e.to_string()))?;

    // Reserve a temp file path under uploads/; the JobId disambiguates
    // concurrent uploaders. We stream the `crate` part directly into
    // this file, hashing with blake3 in flight, then atomically rename
    // to `<blake>.tar` after validation succeeds.
    let tmp_path = sd.uploads_dir.join(format!(".tmp-{job_id}.tar"));
    // Guard so any early return (validation failure, multipart error,
    // mid-stream I/O fault) unlinks the temp file.
    let mut cleanup = TempCleanup::new(tmp_path.clone());

    let mut metadata: Option<PostJobMetadata> = None;
    let mut crate_seen = false;
    let mut hasher = blake3::Hasher::new();

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(multipart_err("multipart"))?
    {
        match field.name() {
            Some("metadata") => {
                if metadata.is_some() {
                    return Err((StatusCode::BAD_REQUEST, "duplicate `metadata` part".into()));
                }
                // Per-field cap: a malicious client could otherwise burn
                // up to `max_upload_bytes` of RAM on a single JSON part.
                // Legitimate metadata is a few hundred bytes.
                const METADATA_MAX_BYTES: usize = 64 * 1024;
                let mut buf: Vec<u8> = Vec::new();
                while let Some(chunk) = field.chunk().await.map_err(multipart_err("metadata"))? {
                    if buf.len().saturating_add(chunk.len()) > METADATA_MAX_BYTES {
                        return Err((
                            StatusCode::PAYLOAD_TOO_LARGE,
                            format!("metadata field exceeds {METADATA_MAX_BYTES} byte cap"),
                        ));
                    }
                    buf.extend_from_slice(&chunk);
                }
                let parsed: PostJobMetadata =
                    serde_json::from_slice(&buf).map_err(bad_request("metadata"))?;
                metadata = Some(parsed);
            }
            Some("crate") => {
                if crate_seen {
                    return Err((StatusCode::BAD_REQUEST, "duplicate `crate` part".into()));
                }
                crate_seen = true;
                let mut file = tokio::fs::File::create(&tmp_path)
                    .await
                    .map_err(|e| internal("create tmp", e.to_string()))?;
                while let Some(chunk) = field.chunk().await.map_err(multipart_err("crate"))? {
                    hasher.update(&chunk);
                    file.write_all(&chunk)
                        .await
                        .map_err(|e| internal("write tmp", e.to_string()))?;
                }
                file.flush()
                    .await
                    .map_err(|e| internal("flush tmp", e.to_string()))?;
            }
            _ => {} // ignore unknown fields silently
        }
    }

    let metadata = metadata.ok_or((StatusCode::BAD_REQUEST, "missing metadata part".into()))?;
    if !crate_seen {
        return Err((StatusCode::BAD_REQUEST, "missing crate part".into()));
    }
    let blake = hasher.finalize().to_hex().to_string();

    // Resolve defaults from config.
    let tcfg = &s.config.timeouts;
    let inactivity_timeout_ms = metadata
        .inactivity_timeout_ms
        .unwrap_or(tcfg.default_inactivity_s * 1_000);
    let hard_max_ms = metadata
        .hard_max_ms
        .unwrap_or(tcfg.default_ad_hoc_hard_max_s * 1_000);
    let daemon_ceiling_ms = tcfg.daemon_ceiling_s * 1_000;

    // Validate selector + ceiling BEFORE rename so a 400 leaves no
    // orphan tar on disk. Inventory snapshot here is informational —
    // the authoritative check runs inside `enqueue_job` under the db
    // lock with a fresh snapshot. The race (board appears/disappears
    // between this check and the enqueue) is acceptable: at worst a
    // valid submit gets a false 400, or a fail-fast 400 slips through
    // and the authoritative check rejects it with the same error class.
    let pre_req = EnqueueRequest {
        job_id,
        priority: metadata.priority,
        submitter: metadata.submitter.clone(),
        // Server forces source = Cli. The wire schema rejects the field
        // entirely (deny_unknown_fields), but we override here too as
        // defense in depth.
        source: JobSource::Cli,
        board_selector: metadata.board_selector.clone(),
        inactivity_timeout_ms,
        hard_max_ms,
        tar_blake3: String::new(),
        tar_path: String::new(),
        daemon_ceiling_ms,
    };
    {
        let inventory = s.inventory_snapshot();
        validate_enqueue(&pre_req, &inventory).map_err(core_to_http)?;
    }

    // Atomically rename .tmp-<jobid>.tar → <blake>.tar.
    //
    // On a dedup hit (`<blake>.tar` already exists) we deliberately
    // leave `cleanup` armed so its Drop unlinks our temp; the existing
    // copy is content-identical and keeps the build cache warm.
    //
    // On both POSIX (`rename(2)`) and Windows (`MoveFileExW` with
    // REPLACE_EXISTING) `tokio::fs::rename` silently clobbers an
    // existing destination, so two concurrent submitters of identical
    // content both succeed; the loser's temp is gone (renamed) and the
    // winner's bytes are content-identical to whatever was clobbered.
    // We therefore do NOT branch on `AlreadyExists` here.
    //
    // No `fsync`: the build cache is allowed to lose tars on a hard
    // crash — on recovery any DB row whose `tar_path` is missing or
    // partial is treated as a fresh blake3 miss and the next submit
    // re-populates it. Crash-consistency is a non-goal for `uploads/`.
    let final_path = sd.uploads_dir.join(format!("{blake}.tar"));
    let final_path_str = path_to_utf8(&final_path)?;
    if !final_path.is_file() {
        tokio::fs::rename(&tmp_path, &final_path)
            .await
            .map_err(|e| internal("rename tmp", e.to_string()))?;
        cleanup.disarm();
    }
    // (else: dedup hit — leave `cleanup` armed; Drop unlinks the temp.)

    // Authoritative enqueue: take a fresh inventory snapshot under the
    // same db lock so there's no TOCTOU between the snapshot and the
    // insert.
    let now_ms = Utc::now().timestamp_millis();
    let req = EnqueueRequest {
        job_id,
        priority: pre_req.priority,
        submitter: pre_req.submitter,
        source: JobSource::Cli,
        board_selector: pre_req.board_selector,
        inactivity_timeout_ms,
        hard_max_ms,
        tar_blake3: blake,
        tar_path: final_path_str,
        daemon_ceiling_ms,
    };
    let inserted = {
        let db = s.db.lock();
        let inventory = s.inventory.lock().clone();
        enqueue_job(db.raw_conn(), &inventory, req, now_ms).map_err(core_to_http)?
    };
    Ok((
        StatusCode::ACCEPTED,
        Json(AcceptedBody {
            job_id: inserted.to_string(),
        }),
    ))
}

/// RAII guard that unlinks `path` on drop unless `disarm` was called.
/// Used to clean up the streaming temp file on any early-return error
/// path (validation failure, multipart error, mid-stream I/O fault).
struct TempCleanup {
    path: Option<std::path::PathBuf>,
}

impl TempCleanup {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path: Some(path) }
    }
    /// Mark the temp file as handled (renamed away). The Drop impl
    /// becomes a no-op.
    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TempCleanup {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            if let Err(e) = std::fs::remove_file(&p) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!(
                        path = %p.display(),
                        error = %e,
                        "failed to clean up temp upload"
                    );
                }
            }
        }
    }
}

fn bad_request<E: std::fmt::Display>(
    stage: &'static str,
) -> impl FnOnce(E) -> (StatusCode, String) {
    move |e| (StatusCode::BAD_REQUEST, format!("{stage}: {e}"))
}

/// Map an `axum::extract::multipart::MultipartError` to an HTTP status.
/// The error type carries the right status itself — most notably
/// `413 Payload Too Large` when the request exceeds the per-route
/// `DefaultBodyLimit`. Defaulting everything to 400 would hide that.
fn multipart_err(
    stage: &'static str,
) -> impl FnOnce(axum::extract::multipart::MultipartError) -> (StatusCode, String) {
    move |e| (e.status(), format!("{stage}: {e}"))
}

fn internal(stage: &'static str, msg: String) -> (StatusCode, String) {
    error!(stage, msg = %msg, "post_jobs internal error");
    (StatusCode::INTERNAL_SERVER_ERROR, msg)
}

fn path_to_utf8(p: &std::path::Path) -> HandlerResult<String> {
    p.to_str().map(|s| s.to_string()).ok_or_else(|| {
        internal(
            "path_to_utf8",
            format!("non-UTF-8 upload path: {}", p.display()),
        )
    })
}

fn core_to_http(e: paavo_core::CoreError) -> (StatusCode, String) {
    use paavo_core::CoreError::*;
    match e {
        SelectorNeverMatches(_) | OverCeiling { .. } | NotCancellable { .. } => {
            (StatusCode::BAD_REQUEST, format!("{e}"))
        }
        Db(_) | Build(_) | Io(_) => {
            error!(error = %e, "post_jobs internal error");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
        }
    }
}

/// GET /jobs?state=...&limit=... — implemented in 4.2.c.ii.
pub async fn list_jobs(_state: State<AppState>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "GET /jobs is wired in 4.2.c.ii",
    )
}

/// GET /jobs/:id — implemented in 4.2.c.ii.
pub async fn get_job(_state: State<AppState>, _id: Path<String>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "GET /jobs/:id is wired in 4.2.c.ii",
    )
}

/// POST /jobs/:id/cancel — implemented in 4.2.c.ii.
pub async fn cancel_job(_state: State<AppState>, _id: Path<String>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "POST /jobs/:id/cancel is wired in 4.2.c.ii",
    )
}

/// GET /jobs/:id/stream — implemented in 4.2.c.iii.
pub async fn stream_job(_state: State<AppState>, _id: Path<String>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "GET /jobs/:id/stream is wired in 4.2.c.iii",
    )
}
