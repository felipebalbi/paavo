//! Tests that the corpus enqueue helper inserts Scheduled jobs and
//! infers `board_kind` from the corpus entry's path basename.

use paavo_db::Db;
use paavo_proto::{BoardHealth, BoardSpec, JobSource, ProbeSelector};
use paavod::app_state::{AppState, DrainState};
use paavod::cancellation::CancellationRegistry;
use paavod::config::{
    BuildCacheConfig, Config, CorpusEntry, QuarantineConfig, RetentionConfig, SchedulerConfig,
    ServerConfig, TimeoutsConfig, WebConfig,
};
use paavod::job_logs::JobLogsBroker;
use parking_lot::Mutex;
use std::sync::Arc;

fn write_test_crate(dir: &std::path::Path, name: &str) {
    let crate_dir = dir.join(name);
    std::fs::create_dir_all(crate_dir.join("src")).unwrap();
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!("[package]\nname=\"{name}\"\nversion=\"0\"\n"),
    )
    .unwrap();
    std::fs::write(crate_dir.join("src/main.rs"), "fn main() {}").unwrap();
}

fn make_state(corpus: Vec<CorpusEntry>, state_root: &std::path::Path) -> AppState {
    let sd = paavod::state_dir::StateDir::from_root(state_root);
    sd.ensure_dirs().unwrap();
    let db = Db::open(&sd.sqlite_path).unwrap();
    paavo_db::BoardRow::insert(
        db.raw_conn(),
        &BoardSpec {
            id: "mcxa266-01".into(),
            kind: "mcxa266".into(),
            probe_selector: ProbeSelector {
                vid: "x".into(),
                pid: "x".into(),
                serial: "x".into(),
            },
            chip_name: "x".into(),
            target_name: "x".into(),
            wiring_profile: None,
            health: BoardHealth::Healthy,
        },
        0,
    )
    .unwrap();
    let inventory = paavo_db::BoardRow::list_all(db.raw_conn())
        .unwrap()
        .into_iter()
        .map(|r| r.spec)
        .collect::<Vec<_>>();
    let cfg = Arc::new(Config {
        server: ServerConfig {
            bind: "127.0.0.1:0".into(),
            state_dir: state_root.to_path_buf(),
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
        corpus,
    });
    AppState {
        db: Arc::new(Mutex::new(db)),
        config: cfg,
        inventory: Arc::new(Mutex::new(inventory)),
        drain: DrainState::default(),
        cancellation: CancellationRegistry::default(),
        job_logs: JobLogsBroker::new(),
    }
}

#[tokio::test]
async fn corpus_entry_enqueues_one_job_per_crate_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    write_test_crate(&corpus_root, "test-a");
    write_test_crate(&corpus_root, "test-b");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );

    paavod::cron::__test_run_once(&state).await.unwrap();
    let rows = paavo_db::JobRow::list_by_state(
        state.db.lock().raw_conn(),
        paavo_proto::JobState::Submitted,
        50,
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.source == JobSource::Scheduler));
    assert!(rows.iter().all(|r| r.board_selector.kind == "mcxa266"));
    assert!(rows
        .iter()
        .all(|r| r.submitter.starts_with("nightly:test-corpus")));
}

#[tokio::test]
async fn corpus_run_skips_non_dir_entries_and_dirs_without_cargo_toml() {
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    std::fs::create_dir_all(&corpus_root).unwrap();
    // A loose file (not a dir) — skipped.
    std::fs::write(corpus_root.join("notes.txt"), "ignored").unwrap();
    // A dir with no Cargo.toml — skipped.
    std::fs::create_dir_all(corpus_root.join("docs/chapters")).unwrap();
    // A real test crate — enqueued.
    write_test_crate(&corpus_root, "test-a");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );

    paavod::cron::__test_run_once(&state).await.unwrap();
    let rows = paavo_db::JobRow::list_by_state(
        state.db.lock().raw_conn(),
        paavo_proto::JobState::Submitted,
        50,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn corpus_run_no_ops_during_drain() {
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    write_test_crate(&corpus_root, "test-a");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );
    state.drain.set_draining();
    paavod::cron::__test_run_once(&state).await.unwrap();
    let rows = paavo_db::JobRow::list_by_state(
        state.db.lock().raw_conn(),
        paavo_proto::JobState::Submitted,
        50,
    )
    .unwrap();
    assert_eq!(rows.len(), 0, "drain must suppress new Scheduled enqueues");
}

#[tokio::test]
async fn corpus_run_updates_schedule_row_with_trigger_and_completion() {
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    write_test_crate(&corpus_root, "test-a");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );
    paavod::cron::__test_run_once(&state).await.unwrap();
    let row = paavo_db::ScheduleRow::get(state.db.lock().raw_conn(), "nightly").unwrap();
    assert!(row.enabled);
    assert!(row.last_triggered_at.is_some());
    assert!(row.last_completed_at.is_some());
    // upsert wrote the cron expression we configured.
    assert_eq!(row.cron, "0 0 19 * * *");
}
