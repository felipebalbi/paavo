//! M7.3 — RealRunner skeleton tests. The probe-rs adapter is NOT yet
//! wired (M7.4-5); these tests cover the DB-read + InfraErr-shape
//! contract so callers see a clear, structured error pointing at the
//! actual elf_path that would have been flashed.

use paavo_core::Runner;
use paavo_db::{Db, JobRow, NewJob};
use paavo_proto::{
    BoardHealth, BoardSelector, BoardSpec, JobId, JobOutcome, JobSource, Priority, ProbeSelector,
    TerminalOutcome,
};
use paavod::cancellation::CancellationRegistry;
use paavod::config::{
    BuildCacheConfig, Config, QuarantineConfig, RetentionConfig, SchedulerConfig, ServerConfig,
    TimeoutsConfig, WebConfig,
};
use paavod::job_logs::JobLogsBroker;
use paavod::real_runner::RealRunner;
use parking_lot::Mutex;
use std::sync::Arc;
use tempfile::TempDir;

/// Build a minimum-viable `Config` + open a fresh DB rooted at `tmp`.
///
/// `Config` has no `Default` impl (most sub-configs require fields), so
/// we construct the literal here. Mirrors `tests/dispatch_loop.rs`'s
/// pattern — keeps the test self-contained instead of widening the
/// production API surface with a test-only constructor that wouldn't
/// be visible from integration tests anyway (`#[cfg(test)]` on a lib
/// item is invisible to `tests/` since they compile as separate crates).
fn boot(tmp: &TempDir) -> (Arc<Mutex<Db>>, Arc<Config>) {
    let db = Db::open(tmp.path().join("p.sqlite")).expect("open db");
    let cfg = Config {
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
    };
    (Arc::new(Mutex::new(db)), Arc::new(cfg))
}

#[test]
fn real_runner_reports_unimplemented_with_elf_path_from_db() {
    let tmp = TempDir::new().unwrap();
    let (db, cfg) = boot(&tmp);

    // Seed a job row already in Running state, with a synthetic
    // elf_path a real RealRunner would have read from.
    let job_id = JobId::new();
    {
        let db = db.lock();
        // FK on job.board_id requires a real board row first.
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
        JobRow::insert(
            db.raw_conn(),
            &NewJob {
                id: job_id,
                priority: Priority::Interactive,
                submitter: "test".into(),
                source: JobSource::Cli,
                board_selector: BoardSelector {
                    kind: "mcxa266".into(),
                    instance: None,
                    wiring_profile: None,
                },
                inactivity_timeout_ms: 120_000,
                hard_max_ms: 900_000,
                tar_blake3: "0".repeat(64),
                tar_path: tmp.path().join("dummy.tar").display().to_string(),
                cargo_update_packages: vec![],
            },
            chrono::Utc::now().timestamp_millis(),
        )
        .unwrap();
        JobRow::transition_to_building(db.raw_conn(), &job_id, "mcxa266-01", 0).unwrap();
        JobRow::transition_to_running(db.raw_conn(), &job_id, "/tmp/elf-from-cache/smoke.elf")
            .unwrap();
    }

    let runner = RealRunner::new(
        db.clone(),
        JobLogsBroker::new(),
        CancellationRegistry::default(),
        cfg.clone(),
    );

    let out = runner.run(job_id, "mcxa266-01");
    match out.outcome {
        JobOutcome::Failed(TerminalOutcome::InfraErr { stage, message }) => {
            assert!(
                stage.contains("real_session") || stage.contains("real_runner"),
                "stage = {stage}"
            );
            assert!(
                message.contains("/tmp/elf-from-cache/smoke.elf"),
                "message must include resolved elf_path. msg = {message}"
            );
        }
        other => panic!("expected InfraErr; got {other:?}"),
    }
    assert!(out.probe_released_cleanly, "no probe touched yet");
}

#[test]
fn real_runner_with_missing_job_row_returns_infraerr_about_missing_job() {
    let tmp = TempDir::new().unwrap();
    let (db, cfg) = boot(&tmp);
    let runner = RealRunner::new(
        db.clone(),
        JobLogsBroker::new(),
        CancellationRegistry::default(),
        cfg.clone(),
    );
    let bogus = JobId::new();

    let out = runner.run(bogus, "mcxa266-01");
    match out.outcome {
        JobOutcome::Failed(TerminalOutcome::InfraErr { stage, message }) => {
            assert!(
                stage.contains("real_runner") || stage.contains("db"),
                "stage = {stage}"
            );
            assert!(
                message.to_lowercase().contains("not found")
                    || message.to_lowercase().contains("missing"),
                "msg = {message}"
            );
        }
        other => panic!("expected InfraErr; got {other:?}"),
    }
}
