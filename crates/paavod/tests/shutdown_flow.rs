//! Tests the drain orchestration directly (signal delivery is wired
//! by paavod::main, not testable cross-platform).

use crossbeam_channel::Receiver;
use paavo_db::Db;
use paavo_proto::{BoardHealth, BoardSpec, JobId, ProbeSelector};
use paavod::app_state::{AppState, DrainState};
use paavod::cancellation::CancellationRegistry;
use paavod::config::{
    BuildCacheConfig, Config, QuarantineConfig, RetentionConfig, SchedulerConfig, ServerConfig,
    TimeoutsConfig, WebConfig,
};
use paavod::job_logs::JobLogsBroker;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

fn make_state(tmp: &std::path::Path) -> AppState {
    let sd = paavod::state_dir::StateDir::from_root(tmp);
    sd.ensure_dirs().unwrap();
    let db = Db::open(&sd.sqlite_path).unwrap();
    let board = BoardSpec {
        id: "b".into(),
        kind: "mcxa266".into(),
        probe_selector: ProbeSelector {
            vid: "x".into(),
            pid: "x".into(),
            serial: "x".into(),
            interface: None,
        },
        chip_name: "x".into(),
        target_name: "x".into(),
        wiring_profile: None,
        health: BoardHealth::Healthy,
    };
    paavo_db::BoardRow::insert(db.raw_conn(), &board, 0).unwrap();
    AppState {
        db: Arc::new(Mutex::new(db)),
        config: Arc::new(Config {
            server: ServerConfig {
                bind: "127.0.0.1:0".into(),
                state_dir: tmp.to_path_buf(),
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
        }),
        inventory: Arc::new(Mutex::new(vec![])),
        drain: DrainState::default(),
        cancellation: CancellationRegistry::default(),
        build_cancel: paavod::cancellation::BuildCancelRegistry::default(),
        job_logs: JobLogsBroker::new(),
    }
}

async fn make_cron(state: AppState) -> paavod::cron::CronHandle {
    paavod::cron::start(state).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn drain_flips_flag_immediately() {
    let tmp = tempfile::tempdir().unwrap();
    let state = make_state(tmp.path());
    let cron = make_cron(state.clone()).await;
    assert!(!state.drain.is_draining());
    let s2 = state.clone();
    let drain_task = tokio::spawn(async move {
        paavod::shutdown::drain_with_grace(s2, cron, Duration::from_millis(100)).await;
    });
    // Yield once so the drain task starts.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        state.drain.is_draining(),
        "drain flag must flip immediately"
    );
    drain_task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn drain_returns_early_when_no_workers_in_flight() {
    let tmp = tempfile::tempdir().unwrap();
    let state = make_state(tmp.path());
    let cron = make_cron(state.clone()).await;
    let start = std::time::Instant::now();
    paavod::shutdown::drain_with_grace(state, cron, Duration::from_secs(5)).await;
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(1),
        "drain with empty registry should return immediately, took {elapsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn drain_signals_remaining_workers_after_grace_expires() {
    // Register a fake worker; never call unregister. Drain should
    // grace-wait, time out, signal DaemonShutdown to the registry
    // entry, and return.
    let tmp = tempfile::tempdir().unwrap();
    let state = make_state(tmp.path());
    let cron = make_cron(state.clone()).await;
    let id = JobId::new();
    state.cancellation.register(id);
    let rx = state.cancellation.take_receiver(&id).unwrap();

    let start = std::time::Instant::now();
    paavod::shutdown::drain_with_grace(state.clone(), cron, Duration::from_millis(200)).await;
    let elapsed = start.elapsed();
    // Should have waited approximately the grace duration.
    assert!(
        elapsed >= Duration::from_millis(200),
        "drain returned before grace expired: {elapsed:?}",
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "drain took too long after grace: {elapsed:?}",
    );
    // The fake worker should have received DaemonShutdown.
    let signalled = recv_with_timeout(&rx, Duration::from_millis(500))
        .expect("worker should receive DaemonShutdown");
    assert_eq!(signalled, paavo_runner::RunCommand::DaemonShutdown);
}

#[tokio::test(flavor = "multi_thread")]
async fn drain_returns_when_worker_finishes_before_grace() {
    // Register a worker, then unregister it on a short delay
    // (simulating the dispatcher's finalize). Drain should poll, see
    // the registry empty, and return well before grace expires.
    let tmp = tempfile::tempdir().unwrap();
    let state = make_state(tmp.path());
    let cron = make_cron(state.clone()).await;
    let id = JobId::new();
    state.cancellation.register(id);
    // Take the rx so the registry behaves like a real dispatch
    // (RealRunner::run takes it before the watchdog reads it).
    // Dropping the rx here is fine — drain only checks the
    // registry's active count, not the rx half.
    let _rx = state.cancellation.take_receiver(&id).unwrap();

    let s2 = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        s2.cancellation.unregister(&id);
    });

    let start = std::time::Instant::now();
    paavod::shutdown::drain_with_grace(state, cron, Duration::from_secs(5)).await;
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(100),
        "drain returned too fast — should have polled at least one cycle: {elapsed:?}",
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "drain should have returned soon after worker unregistered: {elapsed:?}",
    );
}

fn recv_with_timeout(
    rx: &Receiver<paavo_runner::RunCommand>,
    timeout: Duration,
) -> Option<paavo_runner::RunCommand> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(cmd) = rx.try_recv() {
            return Some(cmd);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
