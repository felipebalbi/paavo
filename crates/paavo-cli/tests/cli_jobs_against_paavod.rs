//! Spawns paavod on an ephemeral port, seeds a job, then runs
//! `paavo-cli jobs` and asserts the output contains the job id.

use assert_cmd::Command as AssertCommand;
use paavo_db::Db;
use paavo_proto::{BoardSelector, JobId, JobSource, Priority};
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
async fn paavo_cli_jobs_lists_seeded_job() {
    let tmp = tempdir().unwrap();
    let sd = paavod::state_dir::StateDir::from_root(tmp.path());
    sd.ensure_dirs().unwrap();
    let db = Db::open(&sd.sqlite_path).unwrap();

    let id = JobId::new();
    paavo_db::JobRow::insert(
        db.raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: Priority::Interactive,
            submitter: "felipe".into(),
            source: JobSource::Cli,
            board_selector: BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
        },
        0,
    )
    .unwrap();

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
        .args(["jobs", "--state", "submitted"])
        .assert()
        .success()
        .stdout(predicates::str::contains(id.to_string()));

    server.abort();
}
