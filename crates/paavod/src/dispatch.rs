//! Two-stage dispatch loop.
//!
//! Build stage: claims `Submitted → Building`, bounded by an in-memory
//! slot pool of `max_concurrent_builds` (each slot a dedicated
//! CARGO_TARGET_DIR). On success copies the ELF to a stable
//! content-addressed cache path and moves the job to `AwaitingBoard`,
//! releasing the slot. The board is NOT held during a build.
//!
//! Run stage: claims `AwaitingBoard → Running` only when a matching
//! healthy board is free, then invokes the Runner. Board exclusivity is
//! enforced by `find_healthy_for_selector` (running rows only).
//!
//! Each tick runs the run stage first (keep scarce boards busy), then
//! fills build slots. Drain stops new picks and exits once no build or
//! run is in flight.

use crate::app_state::AppState;
use crate::builder::{BuildOutcome, BuildRequest, Builder};
use crate::job_logs::LiveEvent;
use chrono::Utc;
use paavo_core::{
    apply_outcome_to_board, cache_lookup, cache_store, pick_buildable, pick_runnable, CacheLookup,
    QuarantinePolicy, RunOutcome, Runner, SchedulerConfig,
};
use paavo_db::{JobRow, OutcomeRecord};
use paavo_proto::{AbortReason, JobId, JobOutcome, JobPhase, JobState, TerminalOutcome};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{error, warn};

/// Spawn the dispatch loop. Returns immediately. The loop exits when
/// `state.drain` is set AND no build or run is in flight.
pub fn spawn(
    state: AppState,
    builder: Arc<dyn Builder>,
    runner: Arc<dyn Runner>,
) -> tokio::task::JoinHandle<()> {
    let n = state.config.scheduler.max_concurrent_builds.max(1);
    // Slot pool: a bounded channel pre-filled with slot indices. Acquire
    // = try_recv; release = send back (from the finished build task).
    let (slot_tx, slot_rx) = crossbeam_channel::bounded::<usize>(n);
    for i in 0..n {
        let _ = slot_tx.send(i);
    }
    tokio::spawn(async move {
        loop {
            if state.drain.is_draining() {
                if state.build_cancel.active() == 0 && state.cancellation.active() == 0 {
                    return;
                }
                sleep(Duration::from_millis(100)).await;
                continue;
            }
            let cfg = SchedulerConfig {
                starvation_threshold_ms: state.config.scheduler.starvation_threshold_s * 1_000,
            };
            run_stage(&state, &runner, cfg);
            build_stage(&state, &builder, &slot_rx, &slot_tx, cfg);
            sleep(Duration::from_millis(250)).await;
        }
    })
}

/// Drain `AwaitingBoard` jobs onto free boards until none can dispatch.
fn run_stage(state: &AppState, runner: &Arc<dyn Runner>, cfg: SchedulerConfig) {
    loop {
        let now = Utc::now().timestamp_millis();
        let pick = {
            let db = state.db.lock();
            pick_runnable(db.raw_conn(), cfg, now)
        };
        let scheduled = match pick {
            Ok(Some(s)) => s,
            Ok(None) => break,
            Err(e) => {
                warn!(error = %e, "dispatch: pick_runnable failed");
                break;
            }
        };
        let job_id = scheduled.job.id;
        let board_id = scheduled.board.spec.id.clone();
        let claim_ok = {
            let db = state.db.lock();
            let r = JobRow::transition_awaiting_to_running(db.raw_conn(), &job_id, &board_id);
            if r.is_ok() {
                let _ = paavo_db::BoardRow::touch_last_used(db.raw_conn(), &board_id, now);
            }
            r.is_ok()
        };
        if !claim_ok {
            break; // raced (e.g. a cancel landed); next tick recovers
        }
        state
            .job_logs
            .publish(job_id, LiveEvent::Phase(JobPhase::Running));
        state.cancellation.register(job_id);
        let st = state.clone();
        let rn = runner.clone();
        tokio::task::spawn_blocking(move || run_one_run(st, rn, scheduled.job, board_id));
    }
}

/// Fill free build slots with `Submitted` jobs (single-flight respected
/// by `pick_buildable`).
fn build_stage(
    state: &AppState,
    builder: &Arc<dyn Builder>,
    slot_rx: &crossbeam_channel::Receiver<usize>,
    slot_tx: &crossbeam_channel::Sender<usize>,
    cfg: SchedulerConfig,
) {
    loop {
        let now = Utc::now().timestamp_millis();
        let pick = {
            let db = state.db.lock();
            pick_buildable(db.raw_conn(), cfg, now)
        };
        let job = match pick {
            Ok(Some(j)) => j,
            Ok(None) => break,
            Err(e) => {
                warn!(error = %e, "dispatch: pick_buildable failed");
                break;
            }
        };
        let slot = match slot_rx.try_recv() {
            Ok(s) => s,
            Err(_) => break, // at cap
        };
        let job_id = job.id;
        let claim_ok = {
            let db = state.db.lock();
            JobRow::transition_submitted_to_building(db.raw_conn(), &job_id, now).is_ok()
        };
        if !claim_ok {
            let _ = slot_tx.send(slot);
            break;
        }
        state
            .job_logs
            .publish(job_id, LiveEvent::Phase(JobPhase::Building));
        let cancel_rx = state.build_cancel.register(job_id);
        let st = state.clone();
        let b = builder.clone();
        let stx = slot_tx.clone();
        tokio::task::spawn_blocking(move || run_one_build(st, b, job, slot, stx, cancel_rx));
    }
}

enum BuildStageOutcome {
    /// Job advanced to `AwaitingBoard`; broker stays open for the run.
    Advanced,
    /// Build stage produced a terminal outcome (BuildErr/Aborted/Infra).
    Terminal(JobOutcome),
}

/// Per-job build work on the blocking pool. Always reclaims the slot +
/// build-cancel entry, even on panic.
fn run_one_build(
    state: AppState,
    builder: Arc<dyn Builder>,
    job: JobRow,
    slot: usize,
    slot_tx: crossbeam_channel::Sender<usize>,
    cancel_rx: crossbeam_channel::Receiver<()>,
) {
    let job_id = job.id;
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        build_inner(&state, builder.as_ref(), &job, slot, cancel_rx)
    }));
    let _ = slot_tx.send(slot);
    state.build_cancel.unregister(&job_id);
    match res {
        Ok(BuildStageOutcome::Advanced) => {}
        Ok(BuildStageOutcome::Terminal(outcome)) => finalize_terminal(&state, &job_id, outcome),
        Err(payload) => {
            let message = panic_message(payload);
            error!(%job_id, %message, "dispatch: run_one_build panicked");
            finalize_terminal(
                &state,
                &job_id,
                JobOutcome::Failed(TerminalOutcome::InfraErr {
                    stage: "build_dispatch".into(),
                    message,
                }),
            );
        }
    }
}

fn build_inner(
    state: &AppState,
    builder: &dyn Builder,
    job: &JobRow,
    slot: usize,
    cancel_rx: crossbeam_channel::Receiver<()>,
) -> BuildStageOutcome {
    let sd = crate::state_dir::StateDir::from_root(&state.config.server.state_dir);
    let now = Utc::now().timestamp_millis();

    // Cache hit → straight to AwaitingBoard (no slot work).
    if !job.skip_cache {
        let lookup = {
            let db = state.db.lock();
            cache_lookup(db.raw_conn(), &job.tar_blake3, now)
        };
        if let Ok(CacheLookup::Hit { elf_path }) = lookup {
            let db = state.db.lock();
            return match JobRow::transition_building_to_awaiting_board(
                db.raw_conn(),
                &job.id,
                &elf_path.display().to_string(),
            ) {
                Ok(()) => BuildStageOutcome::Advanced,
                Err(e) => {
                    BuildStageOutcome::Terminal(JobOutcome::Failed(TerminalOutcome::InfraErr {
                        stage: "transition_awaiting_board".into(),
                        message: e.to_string(),
                    }))
                }
            };
        }
    }

    // Build forwarder: cargo lines → broker + log_frame (build phase seq 0..).
    let log_seq = Arc::new(AtomicU64::new(0));
    let job_start = std::time::Instant::now();
    let (build_tx, build_rx) = crossbeam_channel::unbounded::<paavo_build::BuildLine>();
    let mut sink = crate::log_sink::FrameSink::new(
        job.id,
        state.job_logs.clone(),
        state.db.clone(),
        log_seq,
        job_start,
    );
    let fwd = std::thread::Builder::new()
        .name("paavod-build-forwarder".into())
        .spawn(move || loop {
            match build_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(bl) => {
                    let target = match bl.stream {
                        paavo_build::BuildStream::Stdout => "cargo:stdout",
                        paavo_build::BuildStream::Stderr => "cargo:stderr",
                    };
                    sink.push(
                        paavo_proto::LogLevel::Info,
                        Some(target.to_string()),
                        bl.text,
                    );
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => sink.tick(),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    sink.finish();
                    break;
                }
            }
        })
        .expect("spawn paavod-build-forwarder thread");

    let req = BuildRequest {
        job,
        sandbox_dir: sd.sandboxes_dir.join(job.id.to_string()),
        target_dir: sd.build_slot_dir(slot),
    };
    let outcome = builder.build(req, build_tx, cancel_rx);
    let _ = fwd.join();

    match outcome {
        BuildOutcome::Ok { elf_path } => {
            // Copy to a stable, content-addressed artifact so slot reuse
            // can't clobber it and the cache path stays valid.
            let stable = sd.cache_elfs_dir.join(format!("{}.elf", job.tar_blake3));
            if let Err(e) = std::fs::copy(&elf_path, &stable) {
                return BuildStageOutcome::Terminal(JobOutcome::Failed(
                    TerminalOutcome::InfraErr {
                        stage: "artifact_copy".into(),
                        message: e.to_string(),
                    },
                ));
            }
            let stable_str = stable.display().to_string();
            let db = state.db.lock();
            let now2 = Utc::now().timestamp_millis();
            if let Err(e) = cache_store(db.raw_conn(), &job.tar_blake3, &stable, now2) {
                warn!(error = %e, job_id = %job.id, "dispatch: cache_store failed; continuing");
            }
            match JobRow::transition_building_to_awaiting_board(db.raw_conn(), &job.id, &stable_str)
            {
                Ok(()) => BuildStageOutcome::Advanced,
                Err(e) => {
                    BuildStageOutcome::Terminal(JobOutcome::Failed(TerminalOutcome::InfraErr {
                        stage: "transition_awaiting_board".into(),
                        message: e.to_string(),
                    }))
                }
            }
        }
        BuildOutcome::Failed(stderr) => {
            BuildStageOutcome::Terminal(JobOutcome::Failed(TerminalOutcome::BuildErr { stderr }))
        }
        BuildOutcome::Cancelled => BuildStageOutcome::Terminal(JobOutcome::Aborted {
            by: AbortReason::User,
        }),
    }
}

/// Per-job run work on the blocking pool. Continues the log seq space
/// after the build-phase frames so live viewers see one timeline.
fn run_one_run(state: AppState, runner: Arc<dyn Runner>, job: JobRow, board_id: String) {
    let job_id = job.id;
    let start_seq = {
        let db = state.db.lock();
        JobRow::next_log_seq(db.raw_conn(), &job_id).unwrap_or(0)
    };
    let log_seq = Arc::new(AtomicU64::new(start_seq));
    let job_start = std::time::Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runner.run(paavo_core::RunContext {
            job_id,
            board_id: &board_id,
            log_seq: log_seq.clone(),
            job_start,
        })
    }));
    let (outcome, probe_released_cleanly) = match result {
        Ok(RunOutcome {
            outcome,
            probe_released_cleanly,
        }) => (outcome, probe_released_cleanly),
        Err(payload) => {
            let message = panic_message(payload);
            error!(%job_id, %message, "dispatch: run_one_run panicked");
            (
                JobOutcome::Failed(TerminalOutcome::InfraErr {
                    stage: "dispatch".into(),
                    message,
                }),
                false,
            )
        }
    };
    finalize_run(&state, &job_id, &board_id, outcome, probe_released_cleanly);
}

/// Run-stage terminal: persist outcome, apply board quarantine policy,
/// publish Terminal, finalize broker, unregister run cancellation.
fn finalize_run(
    state: &AppState,
    job_id: &JobId,
    board_id: &str,
    outcome: JobOutcome,
    probe_released_cleanly: bool,
) {
    let terminal_state = terminal_state_of(&outcome);
    let now = Utc::now().timestamp_millis();
    {
        let db = state.db.lock();
        if let Err(e) = JobRow::finalize(
            db.raw_conn(),
            job_id,
            &OutcomeRecord {
                state: terminal_state,
                outcome: outcome.clone(),
                finished_at_ms: now,
            },
        ) {
            warn!(error = %e, %job_id, "dispatch: finalize_run failed");
        }
        if let Err(e) = apply_outcome_to_board(
            db.raw_conn(),
            board_id,
            &outcome,
            probe_released_cleanly,
            QuarantinePolicy {
                consecutive_infra_failures: state.config.quarantine.consecutive_infra_failures,
            },
        ) {
            warn!(error = %e, board_id, "dispatch: apply_outcome_to_board failed");
        }
    }
    state
        .job_logs
        .publish(*job_id, LiveEvent::Terminal(outcome));
    state.job_logs.finalize(*job_id);
    state.cancellation.unregister(job_id);
}

/// Build-stage terminal: persist outcome, publish Terminal, finalize
/// broker. No board involved → no quarantine accounting.
fn finalize_terminal(state: &AppState, job_id: &JobId, outcome: JobOutcome) {
    let terminal_state = terminal_state_of(&outcome);
    let now = Utc::now().timestamp_millis();
    {
        let db = state.db.lock();
        if let Err(e) = JobRow::finalize(
            db.raw_conn(),
            job_id,
            &OutcomeRecord {
                state: terminal_state,
                outcome: outcome.clone(),
                finished_at_ms: now,
            },
        ) {
            warn!(error = %e, %job_id, "dispatch: finalize_terminal failed");
        }
    }
    state
        .job_logs
        .publish(*job_id, LiveEvent::Terminal(outcome));
    state.job_logs.finalize(*job_id);
}

fn terminal_state_of(outcome: &JobOutcome) -> JobState {
    match outcome {
        JobOutcome::Passed => JobState::Passed,
        JobOutcome::Failed(_) => JobState::Failed,
        JobOutcome::TimedOut { .. } => JobState::TimedOut,
        JobOutcome::Aborted { .. } => JobState::Aborted,
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}
