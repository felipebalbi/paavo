//! Spins up paavod with two board kinds and asserts `paavo-cli run`
//! (no --board-kind / --instance) fails fast with the ambiguous-kind
//! error, BEFORE it tars or uploads anything. The bogus crate path is
//! intentional: selector resolution runs before crate handling, so the
//! command must error at resolution and never touch the path.

use assert_cmd::Command as AssertCommand;
use paavo_db::Db;
use paavo_proto::{BoardHealth, BoardSpec, ProbeSelector};
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

fn spec(id: &str, kind: &str) -> BoardSpec {
    BoardSpec {
        id: id.into(),
        kind: kind.into(),
        probe_selector: ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: format!("S-{id}"),
        },
        chip_name: "MCXA266".into(),
        target_name: format!("target-{kind}"),
        wiring_profile: None,
        health: BoardHealth::Healthy,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_without_flags_fails_on_ambiguous_kind() {
    let tmp = tempdir().unwrap();
    let sd = paavod::state_dir::StateDir::from_root(tmp.path());
    sd.ensure_dirs().unwrap();
    let db = Db::open(&sd.sqlite_path).unwrap();
    paavo_db::BoardRow::insert(db.raw_conn(), &spec("mcxa266-01", "mcxa266"), 0).unwrap();
    paavo_db::BoardRow::insert(db.raw_conn(), &spec("rt685-01", "rt685"), 0).unwrap();

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
        db: Arc::new(Mutex::new(db)),
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

    AssertCommand::cargo_bin("paavo-cli")
        .unwrap()
        .env("PAAVO_HOST", format!("http://{addr}"))
        .args(["run", "/nonexistent-crate-dir"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("multiple board kinds"));

    server.abort();
}
