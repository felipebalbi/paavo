use paavo_core::{RunOutcome, Runner};
use paavo_db::Db;
use paavo_proto::{
    BoardHealth, BoardSelector, BoardSpec, JobId, JobOutcome, JobSource, JobState, Priority,
    ProbeSelector,
};
use paavod::app_state::{AppState, DrainState};
use paavod::cancellation::CancellationRegistry;
use paavod::config::{
    BuildCacheConfig, Config, QuarantineConfig, RetentionConfig, SchedulerConfig, ServerConfig,
    TimeoutsConfig, WebConfig,
};
use paavod::job_logs::JobLogsBroker;
use paavod::state_dir::StateDir;
use parking_lot::Mutex;
use std::sync::Arc;

struct FakeRunner {
    out: Mutex<JobOutcome>,
}

impl Runner for FakeRunner {
    fn run(&self, _ctx: paavo_core::RunContext<'_>) -> RunOutcome {
        RunOutcome {
            outcome: self.out.lock().clone(),
            probe_released_cleanly: true,
        }
    }
}

fn fixture_state(_out: JobOutcome) -> (AppState, paavo_proto::JobId, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let sd = StateDir::from_root(tmp.path());
    sd.ensure_dirs().unwrap();
    let db = Db::open(&sd.sqlite_path).unwrap();

    // Seed a board.
    let board = BoardSpec {
        id: "b".into(),
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
    };
    paavo_db::BoardRow::insert(db.raw_conn(), &board, 0).unwrap();

    // Pre-populate the build cache so dispatch skips the actual cargo
    // build (we don't want to invoke cargo in unit tests).
    let tar_path = sd.uploads_dir.join("aaa.tar");
    std::fs::write(&tar_path, b"dummy tar bytes").unwrap();
    let elf_path = sd.cache_elfs_dir.join("aaa.elf");
    std::fs::write(&elf_path, b"\x7fELF").unwrap();
    paavo_db::BuildCacheEntry::upsert(
        db.raw_conn(),
        &paavo_db::BuildCacheEntry {
            tar_blake3: "aaa".into(),
            elf_path: elf_path.display().to_string(),
            built_at: 0,
            last_used_at: 0,
            size_bytes: 4,
        },
    )
    .unwrap();

    // Seed a job.
    let job_id = JobId::new();
    paavo_db::JobRow::insert(
        db.raw_conn(),
        &paavo_db::NewJob {
            id: job_id,
            priority: Priority::Interactive,
            submitter: "x".into(),
            source: JobSource::Cli,
            board_selector: BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "aaa".into(),
            tar_path: tar_path.display().to_string(),
            cargo_update_packages: vec![],
            skip_cache: false,
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
    let inventory = paavo_db::BoardRow::list_all(db.raw_conn())
        .unwrap()
        .into_iter()
        .map(|r| r.spec)
        .collect::<Vec<_>>();

    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        config: cfg,
        inventory: Arc::new(Mutex::new(inventory)),
        drain: DrainState::default(),
        cancellation: CancellationRegistry::default(),
        job_logs: JobLogsBroker::new(),
    };
    (state, job_id, tmp)
}

async fn wait_for_terminal(state: &AppState, job_id: &paavo_proto::JobId) -> JobState {
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let row = {
            let db = state.db.lock();
            paavo_db::JobRow::find(db.raw_conn(), job_id).ok().flatten()
        };
        if let Some(r) = row {
            if r.state.is_terminal() {
                return r.state;
            }
        }
    }
    panic!("job did not reach terminal state within 5s");
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatch_runs_a_passed_job_to_completion() {
    let (state, job_id, _tmp) = fixture_state(JobOutcome::Passed);
    let runner: Arc<dyn Runner> = Arc::new(FakeRunner {
        out: Mutex::new(JobOutcome::Passed),
    });
    let handle = paavod::dispatch::spawn(state.clone(), runner);

    assert_eq!(wait_for_terminal(&state, &job_id).await, JobState::Passed);
    let row = {
        let db = state.db.lock();
        paavo_db::JobRow::get(db.raw_conn(), &job_id).unwrap()
    };
    assert_eq!(row.outcome, Some(JobOutcome::Passed));
    assert_eq!(row.board_id.as_deref(), Some("b"));
    // Cancellation registry should be empty after finalize.
    assert_eq!(state.cancellation.active(), 0);

    // Drain shuts the loop down cleanly.
    state.drain.set_draining();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatch_propagates_failed_outcome_and_does_not_quarantine_on_test_err() {
    let outcome = JobOutcome::Failed(paavo_proto::TerminalOutcome::TestErr {
        message: "assertion failed".into(),
    });
    let (state, job_id, _tmp) = fixture_state(outcome.clone());
    let runner: Arc<dyn Runner> = Arc::new(FakeRunner {
        out: Mutex::new(outcome.clone()),
    });
    let handle = paavod::dispatch::spawn(state.clone(), runner);

    assert_eq!(wait_for_terminal(&state, &job_id).await, JobState::Failed);
    let row = {
        let db = state.db.lock();
        paavo_db::JobRow::get(db.raw_conn(), &job_id).unwrap()
    };
    assert_eq!(row.outcome, Some(outcome));
    // TestErr does NOT count toward infra failure (per spec §5.2).
    let board = {
        let db = state.db.lock();
        paavo_db::BoardRow::get(db.raw_conn(), "b").unwrap()
    };
    assert_eq!(board.consecutive_infra_failures, 0);
    assert_eq!(board.spec.health, BoardHealth::Healthy);

    state.drain.set_draining();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatch_emits_terminal_on_job_logs_broker() {
    let (state, job_id, _tmp) = fixture_state(JobOutcome::Passed);
    // Subscribe BEFORE spawning so we capture the Terminal event.
    let mut rx = state.job_logs.subscribe(job_id);
    let runner: Arc<dyn Runner> = Arc::new(FakeRunner {
        out: Mutex::new(JobOutcome::Passed),
    });
    let handle = paavod::dispatch::spawn(state.clone(), runner);
    // Drain non-terminal events (Phase, Frame, …) until the Terminal
    // event arrives. Pre-Phase, this loop saw exactly one event:
    // Terminal. Now that dispatch publishes Phase(Building) and
    // Phase(Running) synchronously with the matching DB transitions,
    // the test's contract is "terminal eventually arrives via the
    // broker", not "Terminal is the first event". The drain is also
    // future-proofing against any variant added later — only the
    // Terminal-shaped event matters for this test.
    let event = loop {
        let next = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timeout waiting for terminal event")
            .expect("broker closed before terminal");
        if matches!(next, paavod::job_logs::LiveEvent::Terminal(_)) {
            break next;
        }
    };
    assert!(matches!(
        event,
        paavod::job_logs::LiveEvent::Terminal(JobOutcome::Passed)
    ));
    state.drain.set_draining();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatch_publishes_phase_events_synchronously_with_state_transitions() {
    // Pin the contract that dispatch emits Phase(Building) and
    // Phase(Running) on the broker, in that order, before the
    // Terminal event. paavo-web's job-detail page (commit 5) drives
    // a phase indicator off these events; without them the
    // indicator would stay stuck on "submitted" through the entire
    // build+run.
    use paavo_proto::JobPhase;
    use paavod::job_logs::LiveEvent;

    let (state, job_id, _tmp) = fixture_state(JobOutcome::Passed);
    let mut rx = state.job_logs.subscribe(job_id);
    let runner: Arc<dyn Runner> = Arc::new(FakeRunner {
        out: Mutex::new(JobOutcome::Passed),
    });
    let handle = paavod::dispatch::spawn(state.clone(), runner);

    // Collect events until we see Terminal, then assert on the
    // sequence. Filtering on "non-Frame" makes the test resilient to
    // future commits that emit per-line build frames into the same
    // broker — Frames may interleave with Phases, but Phase ordering
    // (Building → Running → Terminal) is stable.
    let mut seen_phases: Vec<JobPhase> = Vec::new();
    let mut saw_terminal = false;
    while !saw_terminal {
        let next = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timeout draining broker")
            .expect("broker closed unexpectedly");
        match next {
            LiveEvent::Phase(p) => seen_phases.push(p),
            LiveEvent::Terminal(_) => saw_terminal = true,
            LiveEvent::Frame(_) => {} // ignore — irrelevant to phase ordering
        }
    }

    assert_eq!(
        seen_phases,
        vec![JobPhase::Building, JobPhase::Running],
        "expected Building then Running phases in order; saw: {seen_phases:?}"
    );

    state.drain.set_draining();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatch_exits_loop_on_drain_when_no_jobs_in_flight() {
    let tmp = tempfile::tempdir().unwrap();
    let sd = StateDir::from_root(tmp.path());
    sd.ensure_dirs().unwrap();
    let db = Db::open(&sd.sqlite_path).unwrap();
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
    let runner: Arc<dyn Runner> = Arc::new(FakeRunner {
        out: Mutex::new(JobOutcome::Passed),
    });
    let handle = paavod::dispatch::spawn(state.clone(), runner);
    // Set drain before any work appears. The next poll cycle should
    // exit because nothing is in flight.
    state.drain.set_draining();
    let res = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
    assert!(res.is_ok(), "dispatch loop did not exit on drain");
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatch_does_not_pick_new_jobs_after_drain() {
    // B1 fix: setting drain BEFORE pick_next runs must prevent any
    // claim on a Submitted row. The job should stay Submitted.
    let (state, job_id, _tmp) = fixture_state(JobOutcome::Passed);
    let runner: Arc<dyn Runner> = Arc::new(FakeRunner {
        out: Mutex::new(JobOutcome::Passed),
    });
    // Set drain before spawn — the loop should never claim the job.
    state.drain.set_draining();
    let handle = paavod::dispatch::spawn(state.clone(), runner);
    // Give the loop a few cycles to confirm it didn't claim.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let row = {
        let db = state.db.lock();
        paavo_db::JobRow::get(db.raw_conn(), &job_id).unwrap()
    };
    assert_eq!(
        row.state,
        JobState::Submitted,
        "drain must NOT pick new jobs"
    );
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatch_finalizes_on_runner_panic_without_leaking() {
    // B2 fix: a panic inside runner.run must surface as a terminal
    // outcome (Failed(InfraErr)) and the cancellation registry must
    // be reclaimed.
    struct PanickyRunner;
    impl Runner for PanickyRunner {
        fn run(&self, _ctx: paavo_core::RunContext<'_>) -> RunOutcome {
            panic!("simulated runner panic");
        }
    }

    let (state, job_id, _tmp) = fixture_state(JobOutcome::Passed);
    let runner: Arc<dyn Runner> = Arc::new(PanickyRunner);
    let handle = paavod::dispatch::spawn(state.clone(), runner);

    assert_eq!(wait_for_terminal(&state, &job_id).await, JobState::Failed);
    let row = {
        let db = state.db.lock();
        paavo_db::JobRow::get(db.raw_conn(), &job_id).unwrap()
    };
    match row.outcome {
        Some(paavo_proto::JobOutcome::Failed(paavo_proto::TerminalOutcome::InfraErr {
            stage,
            message,
        })) => {
            assert_eq!(stage, "dispatch");
            assert!(message.contains("simulated runner panic"), "{message}");
        }
        other => panic!("expected InfraErr, got {other:?}"),
    }
    // Cancellation must be reclaimed even on panic path.
    assert_eq!(state.cancellation.active(), 0);

    state.drain.set_draining();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatch_does_not_double_dispatch_same_board() {
    // Hardware-safety invariant from paavo-db's exclusivity clause: two
    // jobs targeting the same kind, only one board available — the
    // second must wait for the first to finish.
    use std::sync::atomic::{AtomicU32, Ordering};

    // FakeRunner that counts concurrent calls and asserts max=1.
    struct Counter {
        concurrent: AtomicU32,
        max_seen: AtomicU32,
    }
    struct CountingRunner {
        counter: Arc<Counter>,
    }
    impl Runner for CountingRunner {
        fn run(&self, _ctx: paavo_core::RunContext<'_>) -> RunOutcome {
            let now = self.counter.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
            self.counter.max_seen.fetch_max(now, Ordering::SeqCst);
            // Hold for 200ms so any double-dispatch is observable.
            std::thread::sleep(std::time::Duration::from_millis(200));
            self.counter.concurrent.fetch_sub(1, Ordering::SeqCst);
            RunOutcome {
                outcome: JobOutcome::Passed,
                probe_released_cleanly: true,
            }
        }
    }

    let (state, job_a, _tmp) = fixture_state(JobOutcome::Passed);
    // Seed a SECOND job targeting the same kind (only one board).
    let job_b = JobId::new();
    let tar_path = state.config.server.state_dir.join("uploads/aaa.tar");
    paavo_db::JobRow::insert(
        state.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id: job_b,
            priority: Priority::Interactive,
            submitter: "x".into(),
            source: JobSource::Cli,
            board_selector: BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "aaa".into(),
            tar_path: tar_path.display().to_string(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();

    let counter = Arc::new(Counter {
        concurrent: AtomicU32::new(0),
        max_seen: AtomicU32::new(0),
    });
    let runner: Arc<dyn Runner> = Arc::new(CountingRunner {
        counter: counter.clone(),
    });
    let handle = paavod::dispatch::spawn(state.clone(), runner);

    // Wait for both jobs to reach terminal.
    assert_eq!(wait_for_terminal(&state, &job_a).await, JobState::Passed);
    assert_eq!(wait_for_terminal(&state, &job_b).await, JobState::Passed);
    assert_eq!(
        counter.max_seen.load(Ordering::SeqCst),
        1,
        "board must never have more than one concurrent dispatch"
    );

    state.drain.set_draining();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}
