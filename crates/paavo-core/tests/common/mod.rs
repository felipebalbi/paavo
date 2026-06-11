#![allow(dead_code)]
//! Shared scaffolding for paavo-core integration tests.
//!
//! Not every test imports every helper. We allow `dead_code` because
//! Rust compiles `mod common;` separately into each `tests/*.rs` binary,
//! so unused helpers in one binary trigger warnings even though they're
//! used in others.

use chrono::Utc;
use paavo_core::{enqueue_job, EnqueueRequest};
use paavo_db::{BoardRow, Db};
use paavo_proto::{
    BoardHealth, BoardSelector, BoardSpec, JobId, JobSource, Priority, ProbeSelector,
};
use tempfile::tempdir;

/// Daemon-side ceiling used by the test suite when validating hard-max
/// requests. 8 hours, matching spec §5 examples.
pub const DAEMON_CEILING_8H_MS: u64 = 8 * 60 * 60 * 1_000;
/// A hard-max value deliberately above `DAEMON_CEILING_8H_MS`, used by
/// over-ceiling rejection tests.
pub const OVER_CEILING_9H_MS: u64 = 9 * 60 * 60 * 1_000;

/// Fresh tempdir-backed `Db` for one test. The TempDir handle is leaked
/// (`std::mem::forget`) so the .sqlite file survives for the test's
/// lifetime; OS-level cleanup (`cargo clean`, reboot) reclaims the dir
/// later. Established pattern across paavo-db's own integration tests —
/// see `crates/paavo-db/tests/board_ops.rs` for precedent.
pub fn fresh_db() -> Db {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    std::mem::forget(dir);
    db
}

/// Insert a board into the `board` table and return the equivalent
/// `BoardSpec`. If `health` is `Quarantined`, also flips the quarantine
/// reason via `BoardRow::quarantine`.
pub fn insert_board(db: &Db, id: &str, kind: &str, health: BoardHealth) -> BoardSpec {
    let spec = BoardSpec {
        id: id.into(),
        kind: kind.into(),
        probe_selector: ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: id.into(),
        },
        chip_name: "X".into(),
        target_name: format!("target-{kind}"),
        wiring_profile: Some("default".into()),
        health,
    };
    BoardRow::insert(db.raw_conn(), &spec, Utc::now().timestamp_millis()).unwrap();
    if health == BoardHealth::Quarantined {
        BoardRow::quarantine(
            db.raw_conn(),
            id,
            "insert_board(): pre-quarantined for test",
        )
        .unwrap();
    }
    spec
}

/// `Vec<BoardSpec>` projection of the board inventory. Scheduler-side
/// columns like `last_used_at` are dropped; enqueue only consumes
/// `BoardSpec` and that's what this helper returns.
pub fn list_inventory_specs(db: &Db) -> Vec<BoardSpec> {
    BoardRow::list_all(db.raw_conn())
        .unwrap()
        .into_iter()
        .map(|r| r.spec)
        .collect()
}

/// A sane-default `EnqueueRequest` for the given board kind. Tests vary
/// only the fields they care about; everything else stays at this baseline
/// so reading any test makes the per-test deviation obvious at a glance.
pub fn default_enqueue_request(kind: &str) -> EnqueueRequest {
    EnqueueRequest {
        job_id: JobId::new(),
        priority: Priority::Interactive,
        submitter: "test".into(),
        source: JobSource::Cli,
        board_selector: BoardSelector {
            kind: kind.into(),
            instance: None,
            wiring_profile: None,
        },
        inactivity_timeout_ms: 120_000,
        hard_max_ms: 900_000,
        tar_blake3: "x".into(),
        tar_path: "/tmp/x.tar".into(),
        daemon_ceiling_ms: DAEMON_CEILING_8H_MS,
        cargo_update_packages: vec![],
    }
}

/// Enqueue a job with one or more field overrides on the default request.
/// Tests pass a closure that mutates the `EnqueueRequest` before insert.
/// Returns the `JobId` so callers can assert on dispatch order.
///
/// ```ignore
/// let id = enqueue_with(&db, NOW, |req| {
///     req.priority = Priority::Scheduled;
///     req.source = JobSource::Scheduler;
/// });
/// ```
pub fn enqueue_with(
    db: &Db,
    submitted_at_ms: i64,
    overrides: impl FnOnce(&mut EnqueueRequest),
) -> JobId {
    let mut req = default_enqueue_request("mcxa266");
    overrides(&mut req);
    let id = req.job_id;
    enqueue_job(
        db.raw_conn(),
        &list_inventory_specs(db),
        req,
        submitted_at_ms,
    )
    .unwrap();
    id
}
