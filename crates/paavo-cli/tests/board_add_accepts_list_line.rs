//! End-to-end proof that `paavo-cli board add --probe "<full probe-rs list
//! line>"` threads the *parsed* selector — including the `-IFACE` interface
//! suffix — all the way through `POST /boards` and into the DB.
//!
//! Harness mirrors `cli_run_board_resolution.rs`: build a `Config`, open a
//! `Db` in a tempdir, wrap it in the same `Arc<Mutex<Db>>` the `AppState`
//! uses, build the router via `paavod::app::build_router`, bind a real
//! `TcpListener`, and `tokio::spawn` `axum::serve` so the daemon is reachable
//! over TCP (the `board add` command performs a real `POST /boards`).
//!
//! Because the served daemon writes through the *same* `Arc<Mutex<Db>>`
//! handle we hold here, we read the stored row straight back with
//! `BoardRow::find` (no extra dependency, no second sqlite connection) and
//! assert the daemon actually persisted the normalized selector.

use assert_cmd::Command as AssertCommand;
use paavo_db::{BoardRow, Db};
use paavod::app::build_router;
use paavod::app_state::{AppState, DrainState};
use paavod::cancellation::CancellationRegistry;
use paavod::config::{
    BuildCacheConfig, Config, QuarantineConfig, RetentionConfig, SchedulerConfig, ServerConfig,
    TimeoutsConfig, WebConfig,
};
use paavod::job_logs::JobLogsBroker;
use parking_lot::Mutex;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn board_add_accepts_full_probe_rs_list_line() {
    let tmp = tempdir().unwrap();
    let sd = paavod::state_dir::StateDir::from_root(tmp.path());
    sd.ensure_dirs().unwrap();

    // Keep our own clone of the shared DB handle so we can read the row back
    // after the CLI's POST commits. The served daemon mutates *this* handle.
    let db = Arc::new(Mutex::new(Db::open(&sd.sqlite_path).unwrap()));

    let cfg = Arc::new(Config {
        server: ServerConfig {
            bind: "127.0.0.1:0".into(),
            state_dir: tmp.path().to_path_buf(),
            max_upload_bytes: 256 * 1024 * 1024,
        },
        web: WebConfig {
            bind: "127.0.0.1:0".into(),
        },
        timeouts: TimeoutsConfig::default(),
        scheduler: SchedulerConfig {
            nightly_cron: "0 0 19 * * *".into(),
            starvation_threshold_s: 21_600,
            max_concurrent_builds: 5,
        },
        build_cache: BuildCacheConfig::default(),
        retention: RetentionConfig::default(),
        quarantine: QuarantineConfig::default(),
        corpus: vec![],
    });
    let state = AppState {
        db: db.clone(),
        config: cfg,
        inventory: Arc::new(Mutex::new(vec![])),
        drain: DrainState::default(),
        cancellation: CancellationRegistry::default(),
        build_cancel: paavod::cancellation::BuildCancelRegistry::default(),
        job_logs: JobLogsBroker::new(),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state);
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Paste a full `probe-rs list` line verbatim. The `-0` after the PID is
    // the USB interface index; the CLI must parse it client-side and ship the
    // structured selector to the daemon.
    AssertCommand::cargo_bin("paavo-cli")
        .unwrap()
        .env("PAAVO_HOST", format!("http://{addr}"))
        .args([
            "board",
            "add",
            "--kind",
            "mcxa266",
            "--instance",
            "mcxa266-77",
            "--probe",
            "[0]: MCU-LINK on-board (r2E4) CMSIS-DAP V3.172 \
             -- 1fc9:0143-0:EDFHUAFM4J5ZJ (CMSIS-DAP)",
            "--chip",
            "MCXA266VFL",
            "--target",
            "frdm-mcx-a266",
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("added: mcxa266-77"));

    // Read the row the daemon just persisted, through the same shared handle.
    let stored = {
        let guard = db.lock();
        BoardRow::find(guard.raw_conn(), "mcxa266-77")
            .unwrap()
            .expect("board mcxa266-77 should have been inserted by POST /boards")
    };

    let sel = &stored.spec.probe_selector;
    assert_eq!(sel.vid, "1fc9");
    assert_eq!(sel.pid, "0143");
    assert_eq!(sel.serial, "EDFHUAFM4J5ZJ");
    assert_eq!(
        sel.interface,
        Some(0),
        "the `-0` interface suffix must survive the CLI -> POST /boards -> DB round-trip"
    );

    server.abort();
}
