//! Integration tests for `POST /admin/purge`.

use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use paavo_db::{Db, JobRow, NewJob};
use paavo_proto::{BoardSelector, BoardSpec, JobId, JobSource, Priority, ProbeSelector};
use paavod::app::build_router;
use paavod::app_state::{AppState, DrainState};
use paavod::config::{
    BuildCacheConfig, Config, QuarantineConfig, RetentionConfig, SchedulerConfig, ServerConfig,
    TimeoutsConfig, WebConfig,
};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;

/// Build an AppState anchored at a real on-disk state_dir so the purge
/// handler can wipe directories. Returns the state_dir alongside the
/// state so tests can inspect the on-disk side-effects. The TempDir
/// handle is leaked so the directory survives for the lifetime of the
/// test (matching the workspace test-fixture convention).
fn state_with_dir() -> (PathBuf, AppState) {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().to_path_buf();
    let sd = paavod::state_dir::StateDir::from_root(&state_dir);
    sd.ensure_dirs().unwrap();
    let db = Db::open(&sd.sqlite_path).unwrap();
    std::mem::forget(dir);
    let cfg = Config {
        server: ServerConfig {
            bind: "127.0.0.1:0".into(),
            state_dir: state_dir.clone(),
            max_upload_bytes: 256 * 1024 * 1024,
        },
        web: WebConfig {
            bind: "127.0.0.1:0".into(),
        },
        timeouts: TimeoutsConfig::default(),
        scheduler: SchedulerConfig {
            nightly_cron: "0 0 19 * * *".into(),
            starvation_threshold_s: 21_600,
        },
        build_cache: BuildCacheConfig::default(),
        retention: RetentionConfig::default(),
        quarantine: QuarantineConfig::default(),
        corpus: vec![],
    };
    let s = AppState {
        db: Arc::new(Mutex::new(db)),
        config: Arc::new(cfg),
        inventory: Arc::new(Mutex::new(vec![])),
        drain: DrainState::default(),
        job_logs: paavod::job_logs::JobLogsBroker::new(),
        cancellation: paavod::cancellation::CancellationRegistry::default(),
    };
    (state_dir, s)
}

fn sample_board() -> BoardSpec {
    BoardSpec {
        id: "mcxa266-01".into(),
        kind: "mcxa266".into(),
        probe_selector: ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: "ABC".into(),
        },
        chip_name: "MCXA266VFL".into(),
        target_name: "frdm-mcx-a266".into(),
        wiring_profile: Some("default".into()),
        health: paavo_proto::BoardHealth::Healthy,
    }
}

fn seed_terminal_job(state: &AppState) -> JobId {
    let id = JobId::new();
    {
        let db = state.db.lock();
        JobRow::insert(
            db.raw_conn(),
            &NewJob {
                id,
                priority: Priority::Interactive,
                submitter: "test".into(),
                source: JobSource::Cli,
                board_selector: BoardSelector {
                    kind: "mcxa266".into(),
                    instance: None,
                    wiring_profile: None,
                },
                inactivity_timeout_ms: 60_000,
                hard_max_ms: 600_000,
                tar_blake3: "x".into(),
                tar_path: "/tmp/x.tar".into(),
                cargo_update_packages: vec![],
                skip_cache: false,
            },
            0,
        )
        .unwrap();
        // Move it through Building → Running → finalize so it lands in
        // a terminal state. transition_to_building needs a board row,
        // so we keep the test simpler by marking it `passed` via a
        // raw UPDATE — the gate is on state, not on transition path.
        db.raw_conn()
            .execute(
                "UPDATE job SET state = 'passed', finished_at = 100 WHERE id = ?1",
                rusqlite::params![id.to_string()],
            )
            .unwrap();
    }
    id
}

fn seed_in_flight_job(state: &AppState) -> JobId {
    let id = JobId::new();
    {
        let db = state.db.lock();
        JobRow::insert(
            db.raw_conn(),
            &NewJob {
                id,
                priority: Priority::Interactive,
                submitter: "test".into(),
                source: JobSource::Cli,
                board_selector: BoardSelector {
                    kind: "mcxa266".into(),
                    instance: None,
                    wiring_profile: None,
                },
                inactivity_timeout_ms: 60_000,
                hard_max_ms: 600_000,
                tar_blake3: "x".into(),
                tar_path: "/tmp/x.tar".into(),
                cargo_update_packages: vec![],
                skip_cache: false,
            },
            0,
        )
        .unwrap();
        db.raw_conn()
            .execute(
                "UPDATE job SET state = 'running', started_at = 100 WHERE id = ?1",
                rusqlite::params![id.to_string()],
            )
            .unwrap();
    }
    id
}

fn seed_board(state: &AppState) -> BoardSpec {
    let spec = sample_board();
    {
        let db = state.db.lock();
        paavo_db::BoardRow::insert(db.raw_conn(), &spec, 0).unwrap();
    }
    spec
}

async fn post_empty(app: axum::Router, uri: &str) -> axum::http::Response<axum::body::Body> {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    app.oneshot(req).await.unwrap()
}

async fn read_text(resp: axum::http::Response<axum::body::Body>) -> String {
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn purge_on_empty_state_returns_204() {
    let (_sd, s) = state_with_dir();
    let app = build_router(s);
    let resp = post_empty(app, "/admin/purge").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn purge_truncates_job_and_log_frame_and_build_cache() {
    let (_sd, s) = state_with_dir();
    let job_id = seed_terminal_job(&s);
    // Seed one log frame and one build_cache row so we can assert
    // truncation rather than just "no error".
    {
        let db = s.db.lock();
        db.raw_conn()
            .execute(
                "INSERT INTO log_frame (job_id, seq, ts_us, level, target, message)
                 VALUES (?1, 0, 0, 'info', NULL, 'hi')",
                rusqlite::params![job_id.to_string()],
            )
            .unwrap();
        db.raw_conn()
            .execute(
                "INSERT INTO build_cache
                    (tar_blake3, elf_path, built_at, last_used_at, size_bytes)
                 VALUES ('x', '/tmp/x.elf', 0, 0, 0)",
                [],
            )
            .unwrap();
    }
    let app = build_router(s.clone());
    let resp = post_empty(app, "/admin/purge").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let db = s.db.lock();
    let n_job: i64 = db
        .raw_conn()
        .query_row("SELECT COUNT(*) FROM job", [], |r| r.get(0))
        .unwrap();
    let n_log: i64 = db
        .raw_conn()
        .query_row("SELECT COUNT(*) FROM log_frame", [], |r| r.get(0))
        .unwrap();
    let n_cache: i64 = db
        .raw_conn()
        .query_row("SELECT COUNT(*) FROM build_cache", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n_job, 0);
    assert_eq!(n_log, 0);
    assert_eq!(n_cache, 0);
}

#[tokio::test]
async fn purge_preserves_boards_and_schedules() {
    let (_sd, s) = state_with_dir();
    let spec = seed_board(&s);
    {
        let db = s.db.lock();
        paavo_db::ScheduleRow::upsert(
            db.raw_conn(),
            &paavo_db::ScheduleRow {
                id: "nightly".into(),
                cron: "0 0 19 * * *".into(),
                enabled: true,
                last_triggered_at: None,
                last_completed_at: None,
            },
        )
        .unwrap();
    }
    let app = build_router(s.clone());
    let resp = post_empty(app, "/admin/purge").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let db = s.db.lock();
    let board_row = paavo_db::BoardRow::get(db.raw_conn(), &spec.id).unwrap();
    assert_eq!(board_row.spec.id, spec.id);
    let sched_row = paavo_db::ScheduleRow::get(db.raw_conn(), "nightly").unwrap();
    assert_eq!(sched_row.cron, "0 0 19 * * *");
}

#[tokio::test]
async fn purge_refuses_with_409_when_job_is_running() {
    let (_sd, s) = state_with_dir();
    let _id = seed_in_flight_job(&s);
    let app = build_router(s.clone());
    let resp = post_empty(app, "/admin/purge").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = read_text(resp).await;
    assert!(body.contains("building or running"), "got: {body}");
    // Confirm nothing was wiped: the in-flight job is still there.
    let db = s.db.lock();
    let n: i64 = db
        .raw_conn()
        .query_row("SELECT COUNT(*) FROM job", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn purge_wipes_sandboxes_and_uploads_and_cargo_target() {
    let (sd, s) = state_with_dir();
    // Drop some real files into each on-disk dir so we can verify the
    // wipe actually swept them. Mirrors the layout paavod produces in
    // production: sandboxes/<job_id>/Cargo.toml, uploads/<hash>.tar,
    // cargo-target/<triple>/release/...
    let sandboxes = sd.join("sandboxes").join("01JOBID");
    std::fs::create_dir_all(&sandboxes).unwrap();
    std::fs::write(sandboxes.join("Cargo.toml"), b"placeholder").unwrap();
    let uploads = sd.join("uploads");
    std::fs::write(uploads.join("blake3hash.tar"), b"tarbytes").unwrap();
    let cargo_target = sd.join("cargo-target").join("thumbv8m");
    std::fs::create_dir_all(&cargo_target).unwrap();
    std::fs::write(cargo_target.join("artifact.elf"), b"\x7fELF").unwrap();

    let app = build_router(s);
    let resp = post_empty(app, "/admin/purge").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // The top-level dirs themselves must survive (paavod expects them
    // to exist on next job submit). Only their contents go.
    assert!(sd.join("sandboxes").is_dir(), "sandboxes/ removed");
    assert!(sd.join("uploads").is_dir(), "uploads/ removed");
    assert!(sd.join("cargo-target").is_dir(), "cargo-target/ removed");
    assert!(!sandboxes.exists(), "leftover sandbox dir survived");
    assert!(
        !uploads.join("blake3hash.tar").exists(),
        "leftover tar survived"
    );
    assert!(!cargo_target.exists(), "leftover cargo-target dir survived");
}

#[tokio::test]
async fn purge_serves_during_drain() {
    let (_sd, s) = state_with_dir();
    s.drain.set_draining();
    let app = build_router(s);
    let resp = post_empty(app, "/admin/purge").await;
    // Drain semantics in §6.3 / §9.5 gate POST /jobs only; admin must
    // remain reachable so the operator can recover state during a
    // shutdown.
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}
