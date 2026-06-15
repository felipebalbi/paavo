//! Dispatch loop. Polls `pick_next` and runs jobs end-to-end.
//!
//! For each picked job:
//! 1. (Synchronously in the dispatch thread) Transition row to
//!    `Building`, touch the board's `last_used_at`, register a fresh
//!    cancellation sender. Claiming-before-spawn means a re-pick on
//!    the next iteration can't see the same Submitted row.
//! 2. (On the blocking pool) Cache lookup or `paavo_build::build_release`.
//! 3. Transition to `Running`.
//! 4. Call `Runner::run`.
//! 5. Persist outcome, apply quarantine policy, publish terminal event,
//!    unregister cancellation, drop sandbox.
//!
//! Drain ordering (M4.3.d wires the actual SIGTERM hookup): when
//! `state.drain.is_draining()` AND `state.cancellation.active() == 0`,
//! the loop returns. Tasks already on the blocking pool finish on
//! their own; new ones don't get picked.

use crate::app_state::AppState;
use crate::job_logs::LiveEvent;
use chrono::Utc;
use paavo_build::tar::unpack_into;
use paavo_build::BuildPlan;
use paavo_core::{
    apply_outcome_to_board, cache_lookup, cache_store, pick_next, CacheLookup, QuarantinePolicy,
    RunOutcome, Runner, SchedulerConfig,
};
use paavo_db::{JobRow, OutcomeRecord};
use paavo_proto::{JobId, JobOutcome, JobState, TerminalOutcome};
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{error, warn};

/// Spawn the dispatch loop. Returns immediately. The loop exits when
/// `state.drain` flips to true *and* `state.cancellation` reports zero
/// in-flight workers.
///
/// Drain semantics: as soon as drain is set, the loop STOPS picking
/// new Submitted rows. Already-running workers finish on their own,
/// and once the registry empties the loop returns. This is the
/// guarantee §6.3 wants — SIGTERM may not silently start new builds.
pub fn spawn(state: AppState, runner: Arc<dyn Runner>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            // Drain short-circuit: stop picking new jobs the moment
            // drain is set. Exit only when in-flight count is 0.
            if state.drain.is_draining() {
                if state.cancellation.active() == 0 {
                    return;
                }
                sleep(Duration::from_millis(100)).await;
                continue;
            }

            let now_ms = Utc::now().timestamp_millis();
            let pick = {
                let db = state.db.lock();
                pick_next(
                    db.raw_conn(),
                    SchedulerConfig {
                        starvation_threshold_ms: state.config.scheduler.starvation_threshold_s
                            * 1_000,
                    },
                    now_ms,
                )
            };
            let scheduled = match pick {
                Ok(Some(s)) => s,
                Ok(None) => {
                    sleep(Duration::from_millis(250)).await;
                    continue;
                }
                Err(e) => {
                    warn!(error = %e, "dispatch: pick_next failed");
                    sleep(Duration::from_millis(250)).await;
                    continue;
                }
            };

            let job_id = scheduled.job.id;
            let board_id = scheduled.board.spec.id.clone();
            // Claim the job + register cancellation BEFORE spawning the
            // blocking task. Otherwise the next loop iteration would
            // pick_next the same Submitted row.
            let claim_ok = {
                let db = state.db.lock();
                let r = JobRow::transition_to_building(db.raw_conn(), &job_id, &board_id, now_ms);
                if r.is_ok() {
                    let _ = paavo_db::BoardRow::touch_last_used(db.raw_conn(), &board_id, now_ms);
                }
                r.is_ok()
            };
            if !claim_ok {
                // Someone else (a parallel dispatcher in the future, or
                // a manual UPDATE) won the claim race. Brief sleep so we
                // don't spin on the SQLite lock when the race becomes
                // possible.
                sleep(Duration::from_millis(50)).await;
                continue;
            }
            // Allocate a fresh cancel channel inside the registry. The
            // sender stays on the registry entry so `signal()` keeps
            // working through the job's lifetime; the receiver is
            // consumed later by `RealRunner::run` via `take_receiver`,
            // which hands it to the BoardWorker's watchdog. Dispatch
            // itself never touches the rx — wiring is centralised in
            // the runner.
            state.cancellation.register(job_id);

            let state_clone = state.clone();
            let runner_clone = runner.clone();
            tokio::task::spawn_blocking(move || {
                run_one(state_clone, runner_clone, scheduled.job, scheduled.board);
            });
        }
    })
}

/// Per-job blocking work: build (or cache hit), transition to Running,
/// invoke the Runner, persist outcome, apply quarantine, publish
/// terminal frame, unregister cancellation.
///
/// **Cleanup invariant**: every exit path from this function MUST
/// finalize via `finalize_with_outcome` so the cancellation registry
/// and the live-log broker entry are reclaimed. We use
/// `std::panic::catch_unwind` to ensure even a panicking
/// `runner.run` / `build_or_cache` doesn't leak — a panic surfaces as
/// `JobOutcome::Failed(InfraErr { stage: "dispatch", message })` so
/// the operator sees a clear terminal instead of a stuck Building/
/// Running row.
///
/// The cancel-channel rx is owned by the `CancellationRegistry`
/// entry that `spawn` allocated via `register(job_id)`; the runner
/// consumes it via `take_receiver` inside `runner.run`. Dispatch
/// does not touch the rx directly — wiring is centralised in
/// `RealRunner::run`.
fn run_one(
    state: AppState,
    runner: Arc<dyn Runner>,
    job: paavo_db::JobRow,
    board: paavo_db::BoardRow,
) {
    let job_id = job.id;
    let board_id = board.spec.id.clone();

    // Use catch_unwind so a panic in build_or_cache or runner.run can't
    // leave the job stuck in Building/Running with a leaked broker +
    // cancellation entry. The catch boundary is the function body
    // minus its own cleanup. UnwindSafe is satisfied because the only
    // captured-by-mutable-ref values (the inner Option<...> outcome
    // accumulator) are not unwind-observable.
    let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_one_inner(&state, runner.as_ref(), &job, &board_id)
    }));
    let (outcome, probe_released_cleanly) = match panic_result {
        Ok(pair) => pair,
        Err(payload) => {
            let message = panic_message(&payload);
            error!(%job_id, %message, "dispatch: run_one panicked");
            (
                JobOutcome::Failed(TerminalOutcome::InfraErr {
                    stage: "dispatch".into(),
                    message,
                }),
                false,
            )
        }
    };
    finalize_with_outcome(&state, &job_id, &board_id, outcome, probe_released_cleanly);
}

/// Inner body of `run_one` — returns the outcome + probe_released_cleanly
/// pair. Any early return funnels through here and the wrapper in
/// `run_one` guarantees finalize gets called.
fn run_one_inner(
    state: &AppState,
    runner: &dyn Runner,
    job: &paavo_db::JobRow,
    board_id: &str,
) -> (JobOutcome, bool) {
    let job_id = job.id;

    // 2. Cache lookup or build. The cache is keyed by tar_blake3, so
    //    an identical resubmit hits it (fast turnaround). Operators
    //    chasing a flaky chip can force a fresh build + flash by
    //    submitting with `--skip-cache` (paavo-cli) / `skip_cache:
    //    true` (JobSpec); see `JobRow::skip_cache`.
    let elf_path = match build_or_cache(state, job) {
        Ok(p) => p,
        Err(stderr) => {
            return (
                JobOutcome::Failed(TerminalOutcome::BuildErr { stderr }),
                true,
            );
        }
    };

    // 3. Transition Building → Running. If the row has been moved out
    // from under us (e.g. a future cancel-during-Building lands on it),
    // surface as InfraErr so the operator sees a terminal instead of
    // a stuck Building row.
    {
        let db = state.db.lock();
        if let Err(e) =
            JobRow::transition_to_running(db.raw_conn(), &job_id, &elf_path.display().to_string())
        {
            warn!(error = %e, %job_id, "dispatch: transition_to_running failed");
            return (
                JobOutcome::Failed(TerminalOutcome::InfraErr {
                    stage: "transition_to_running".into(),
                    message: e.to_string(),
                }),
                true,
            );
        }
    }

    // 4. Run.
    let RunOutcome {
        outcome,
        probe_released_cleanly,
    } = runner.run(job_id, board_id);
    (outcome, probe_released_cleanly)
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// Look up the cached ELF or build from the tar. On cache miss, run
/// `paavo_build::build_release` and store the result in the cache.
/// Returns the ELF path. On build failure returns the captured
/// stderr tail so the caller can produce a `BuildErr` outcome.
fn build_or_cache(state: &AppState, job: &paavo_db::JobRow) -> Result<std::path::PathBuf, String> {
    let now_ms = Utc::now().timestamp_millis();
    // `skip_cache` is the operator's explicit "rebuild, don't reuse a
    // cached ELF for this tar_blake3" knob (set via paavo-cli
    // `--skip-cache` / JobSpec::skip_cache). When true we deliberately
    // do NOT consult the build_cache table; the cargo target dir is
    // still shared across jobs (CARGO_TARGET_DIR) so a no-op rebuild
    // is fast, but the path goes through `paavo_build::build_release`
    // every time so any drift between submits is healed.
    //
    // We also do NOT delete the existing cache row — a subsequent
    // submit without --skip-cache should still hit the cache. The
    // upstream cache_store at the end of this function refreshes the
    // row's last_used_at + elf_path either way, so the cache stays
    // warm for the next operator who wants the fast path.
    if !job.skip_cache {
        let lookup = {
            let db = state.db.lock();
            cache_lookup(db.raw_conn(), &job.tar_blake3, now_ms)
        };
        if let Ok(CacheLookup::Hit { elf_path }) = lookup {
            return Ok(elf_path);
        }
    } else {
        tracing::info!(
            job_id = %job.id,
            tar_blake3 = %job.tar_blake3,
            "build: skip_cache=true; bypassing build_cache lookup, will rebuild"
        );
    }

    // Cache miss (or lookup error — fall through to a fresh build).
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(&job.tar_path)
        .and_then(|mut f| f.read_to_end(&mut bytes))
        .map_err(|e| format!("read tar {}: {e}", job.tar_path))?;
    let sd = crate::state_dir::StateDir::from_root(&state.config.server.state_dir);
    let crate_dir = sd.sandboxes_dir.join(job.id.to_string());
    unpack_into(&bytes, &crate_dir).map_err(|e| e.to_string())?;

    // Find the unique sub-dir under crate_dir that contains Cargo.toml.
    let crate_root = walkdir::WalkDir::new(&crate_dir)
        .min_depth(1)
        .max_depth(2)
        .into_iter()
        .flatten()
        .find(|e| e.file_name() == "Cargo.toml")
        .map(|e| e.path().parent().unwrap().to_path_buf())
        .ok_or_else(|| "no Cargo.toml in uploaded tar".to_string())?;

    let plan = BuildPlan {
        crate_dir: crate_root,
        target_dir: sd.cargo_target_dir.clone(),
        // Thread the job's `cargo_update_packages` through to the build
        // sandbox. paavo_build runs `cargo update -p <pkg>` for each
        // before `cargo build`. HTTP-submitted jobs always carry
        // `vec![]` (dep graph locked at submit time); the nightly cron
        // populates it from `[[corpus]].cargo_update` so soak runs
        // pull fresh embassy revisions (spec §8.1 step 4).
        cargo_update_packages: job.cargo_update_packages.clone(),
    };
    let res = paavo_build::build_release(&plan).map_err(|e| e.to_string())?;
    let now_ms = Utc::now().timestamp_millis();
    {
        let db = state.db.lock();
        if let Err(e) = cache_store(db.raw_conn(), &job.tar_blake3, &res.elf_path, now_ms) {
            warn!(error = %e, job_id = %job.id, "dispatch: cache_store failed; continuing anyway");
        }
    }
    Ok(res.elf_path)
}

/// Persist the terminal outcome, apply the quarantine policy, publish
/// the Terminal event on the live broker, finalize the broker channel,
/// and unregister the cancellation entry. Idempotent at the broker
/// layer — `finalize` removes any stale Sender. At the DB layer, a
/// second call would fail the WHERE-state-clause guard in
/// `JobRow::finalize`, which is logged + tolerated.
fn finalize_with_outcome(
    state: &AppState,
    job_id: &JobId,
    board_id: &str,
    outcome: JobOutcome,
    probe_released_cleanly: bool,
) {
    let terminal_state = match &outcome {
        JobOutcome::Passed => JobState::Passed,
        JobOutcome::Failed(_) => JobState::Failed,
        JobOutcome::TimedOut { .. } => JobState::TimedOut,
        JobOutcome::Aborted { .. } => JobState::Aborted,
    };
    let now_ms = Utc::now().timestamp_millis();
    {
        let db = state.db.lock();
        if let Err(e) = JobRow::finalize(
            db.raw_conn(),
            job_id,
            &OutcomeRecord {
                state: terminal_state,
                outcome: outcome.clone(),
                finished_at_ms: now_ms,
            },
        ) {
            warn!(error = %e, %job_id, "dispatch: JobRow::finalize failed");
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
