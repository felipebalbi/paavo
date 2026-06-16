//! M7.6 — PAAVO_HW=1 end-to-end test for the closed loop:
//! paavod → RealRunner → paavo_runner::run_job → RealSession →
//! real MCX-A266 EVK → `JobOutcome::Passed`.
//!
//! The other RealRunner tests stub the probe layer; this one closes
//! the loop. Gated two ways (mirrors the M7.4/7.5 hardware tests
//! under `crates/paavo-probe/tests/`):
//!   - `#[ignore]` so the default `cargo test --workspace` skips it.
//!   - `PAAVO_HW=1` env var so even when run with `--ignored`, dev boxes
//!     without the EVK plugged in self-skip without surfacing as failure.
//!
//! Depends on the spike fixture ELF; build it first by `cd`-ing INTO
//! the fixture directory (the `.cargo/config.toml` there carries the
//! `-Tdefmt.x` linker flag — building via `--manifest-path` from the
//! workspace root silently drops `.defmt` and the run will fail at
//! `Table::parse`):
//!
//!   cd dev/spike-fixture-mcxa266
//!   cargo build --release
//!
//! Run with:
//!   $env:PAAVO_HW = "1"
//!   cargo test -p paavod --test real_runner_end_to_end -- --ignored --nocapture

use paavo_core::Runner;
use paavo_db::{Db, JobRow, NewJob};
use paavo_proto::{
    BoardHealth, BoardSelector, BoardSpec, JobId, JobOutcome, JobSource, Priority, ProbeSelector,
};
use paavod::cancellation::CancellationRegistry;
use paavod::config::{
    BuildCacheConfig, Config, QuarantineConfig, RetentionConfig, SchedulerConfig, ServerConfig,
    TimeoutsConfig, WebConfig,
};
use paavod::job_logs::JobLogsBroker;
use paavod::real_runner::RealRunner;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn hw_or_skip() -> bool {
    if std::env::var("PAAVO_HW").is_err() {
        eprintln!("PAAVO_HW not set; skipping hardware test");
        return false;
    }
    true
}

fn elf_fixture() -> PathBuf {
    let here = std::env::current_dir().expect("cwd");
    let repo = here
        .ancestors()
        .find(|p| p.join("dev/spike-fixture-mcxa266/Cargo.toml").is_file())
        .expect("can't find repo root from CWD");
    let elf = repo.join(
        "dev/spike-fixture-mcxa266/target/thumbv8m.main-none-eabihf/release/spike-fixture-mcxa266",
    );
    assert!(
        elf.is_file(),
        "spike fixture ELF not built. Build it FROM INSIDE the fixture dir \
         (the .cargo/config.toml there carries the -Tdefmt.x linker flag; \
         building via --manifest-path from elsewhere drops it and produces \
         an ELF with no .defmt section):\n  \
         cd {}/dev/spike-fixture-mcxa266 && cargo build --release",
        repo.display()
    );
    elf
}

/// Build a minimum-viable `Config` + open a fresh DB rooted at `tmp`.
/// Mirrors `tests/real_runner_skeleton.rs`'s helper before its
/// deletion — keeps the test self-contained instead of widening the
/// production API surface with a test-only constructor.
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
            max_concurrent_builds: 5,
        },
        build_cache: BuildCacheConfig::default(),
        retention: RetentionConfig::default(),
        quarantine: QuarantineConfig::default(),
        corpus: vec![],
    };
    (Arc::new(Mutex::new(db)), Arc::new(cfg))
}

#[test]
#[ignore]
fn real_runner_passes_against_real_mcxa266() {
    if !hw_or_skip() {
        return;
    }

    let tmp = TempDir::new().unwrap();
    let (db, cfg) = boot(&tmp);
    let elf = elf_fixture();

    let board_id = "mcxa266-felipe";
    let job_id = JobId::new();
    {
        let db = db.lock();
        // Seed a board matching Felipe's MCX-A266 EVK. The chip name
        // `MCXA276` (NOT MCXA266) is per the spike finding — probe-rs
        // 0.27 only registers the 276 variant; the 266 is silicon-
        // compatible and works fine under the 276 ID.
        paavo_db::BoardRow::insert(
            db.raw_conn(),
            &BoardSpec {
                id: board_id.into(),
                kind: "mcxa266".into(),
                probe_selector: ProbeSelector {
                    vid: "1fc9".into(),             // NXP
                    pid: "0143".into(),             // MCU-Link CMSIS-DAP
                    serial: "EDFHUAFM4J5ZJ".into(), // Felipe's specific EVK
                },
                chip_name: "MCXA276".into(),
                target_name: "frdm-mcx-a266".into(),
                wiring_profile: None,
                health: BoardHealth::Healthy,
            },
            0,
        )
        .unwrap();

        // Insert a job, then transition it through Building → Running
        // so its elf_path column is populated (mirrors what dispatch
        // does for a cache-hit job).
        JobRow::insert(
            db.raw_conn(),
            &NewJob {
                id: job_id,
                priority: Priority::Interactive,
                submitter: "real_runner_end_to_end".into(),
                source: JobSource::Cli,
                board_selector: BoardSelector {
                    kind: "mcxa266".into(),
                    instance: None,
                    wiring_profile: None,
                },
                inactivity_timeout_ms: 30_000,
                hard_max_ms: 60_000,
                tar_blake3: "0".repeat(64),
                tar_path: tmp.path().join("dummy.tar").display().to_string(),
                cargo_update_packages: vec![],
                skip_cache: false,
            },
            chrono::Utc::now().timestamp_millis(),
        )
        .unwrap();
        JobRow::transition_to_building(db.raw_conn(), &job_id, board_id, 0).unwrap();
        JobRow::transition_to_running(db.raw_conn(), &job_id, &elf.display().to_string()).unwrap();
    }

    // Register a cancellation entry. RealRunner's run() looks up
    // `take_receiver(&job_id)` so the worker's watchdog can read
    // cancel signals. Missing-entry is treated as an InfraErr
    // (the cancel path would otherwise be silently dead), so this
    // register call is required not optional.
    let cancellation = CancellationRegistry::default();
    cancellation.register(job_id);

    let runner = RealRunner::new(db.clone(), JobLogsBroker::new(), cancellation, cfg.clone());
    let outcome = runner.run(paavo_core::RunContext {
        job_id,
        board_id,
        log_seq: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        job_start: std::time::Instant::now(),
    });

    assert_eq!(
        outcome.outcome,
        JobOutcome::Passed,
        "expected Passed, got {:?}",
        outcome.outcome
    );
    assert!(
        outcome.probe_released_cleanly,
        "probe should release cleanly on a Passed run"
    );
}
