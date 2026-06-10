//! BoardWorker entry point. `run_job` takes a probe session (real or mock),
//! drives it until either the test reports done (`Test OK` + bkpt → pass,
//! panic frame → fail) or the watchdog fires.

use crate::job::{JobInputs, JobOutputs};
use crate::watchdog::{run_watchdog, StopReason, WatchdogState};
use crossbeam_channel::{bounded, Sender};
use paavo_probe::{Event, ProbeSession};
use paavo_proto::{AbortReason, JobOutcome, LogFrame, LogLevel, TerminalOutcome, TimeoutReason};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Handle to a spawned BoardWorker thread. Drop blocks the caller until the
/// thread exits.
pub struct BoardWorkerHandle {
    join: Option<thread::JoinHandle<JobOutcome>>,
}

impl BoardWorkerHandle {
    /// Wait for the worker to finish and return the terminal outcome.
    pub fn join(mut self) -> JobOutcome {
        self.join
            .take()
            .expect("join already called")
            .join()
            .expect("BoardWorker panicked")
    }
}

impl Drop for BoardWorkerHandle {
    fn drop(&mut self) {
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Run a job to terminal outcome.
///
/// `make_session` is called on the worker thread (so probe-rs runs off the
/// caller's thread). The mock-session test path supplies a closure that
/// returns a `Box<dyn ProbeSession>` wrapping a deterministic event source.
pub fn run_job<F>(inputs: JobInputs, outputs: JobOutputs, make_session: F) -> BoardWorkerHandle
where
    F: FnOnce() -> paavo_probe::Result<Box<dyn ProbeSession>> + Send + 'static,
{
    let JobInputs {
        job_id: _,
        inactivity_timeout_ms,
        hard_max_ms,
        probe_release_grace_ms,
        cancel_rx,
    } = inputs;
    let JobOutputs { log_tx } = outputs;

    let join = thread::Builder::new()
        .name("paavo-board-worker".into())
        .spawn(move || -> JobOutcome {
            let state = WatchdogState::new(Instant::now());

            let (worker_done_tx, worker_done_rx) = bounded::<()>(1);
            let watchdog_state = state.clone();
            let watchdog_inactivity = Duration::from_millis(inactivity_timeout_ms);
            let watchdog_hardmax = Duration::from_millis(hard_max_ms);
            let watchdog_join = thread::Builder::new()
                .name("paavo-watchdog".into())
                .spawn(move || {
                    run_watchdog(
                        watchdog_state,
                        watchdog_inactivity,
                        watchdog_hardmax,
                        cancel_rx,
                        Duration::from_millis(100),
                        worker_done_rx,
                    )
                })
                .expect("spawn watchdog");

            let session = match make_session() {
                Ok(s) => {
                    // Probe attach + flash + RTT-init can take seconds. Reset
                    // the inactivity clock so the watchdog starts measuring
                    // from the moment the session is live, not from thread
                    // spawn. Hard-max still measures from spawn (spec §6.2).
                    state.touch(Instant::now());
                    s
                }
                Err(e) => {
                    // Tell the watchdog to exit, then join it.
                    let _ = worker_done_tx.send(());
                    let _ = watchdog_join.join();
                    return JobOutcome::Failed(TerminalOutcome::InfraErr {
                        stage: "probe_attach".into(),
                        message: format!("{e}"),
                    });
                }
            };
            let outcome = drive_session(
                session,
                state.clone(),
                &log_tx,
                Duration::from_millis(probe_release_grace_ms),
            );
            // Tell the watchdog to exit (no-op if it already fired).
            let _ = worker_done_tx.send(());
            let _ = watchdog_join.join();
            outcome
        })
        .expect("spawn worker");

    BoardWorkerHandle { join: Some(join) }
}

fn drive_session(
    mut session: Box<dyn ProbeSession>,
    state: Arc<WatchdogState>,
    log_tx: &Sender<LogFrame>,
    release_grace: Duration,
) -> JobOutcome {
    let mut seen_test_ok = false;
    loop {
        // Watchdog-fired stop takes priority over everything.
        if let Some(reason) = state.stop_reason() {
            return finalise_for_stop(reason, state.started_at, release_grace, &mut session);
        }

        match session.next_event(/* timeout_ms = */ 500) {
            Ok(Some(Event::LogFrame(frame))) => {
                state.touch(Instant::now());
                // Pass detection: an info-level frame whose body, after
                // trimming, is exactly `Test OK`, followed by `Bkpt`. Exact
                // match (not `contains`) avoids false positives from log
                // messages that happen to include the marker substring.
                if frame.level == LogLevel::Info && frame.message.trim() == "Test OK" {
                    seen_test_ok = true;
                }
                let _ = log_tx.send(frame);
            }
            Ok(Some(Event::Bkpt)) => {
                if seen_test_ok {
                    return JobOutcome::Passed;
                }
                // bkpt without Test OK marker → treat as test error.
                return JobOutcome::Failed(TerminalOutcome::TestErr {
                    message: "bkpt without preceding Test OK".into(),
                });
            }
            Ok(Some(Event::Panic { message })) => {
                return JobOutcome::Failed(TerminalOutcome::TestErr { message });
            }
            Ok(Some(Event::Disconnect)) => {
                return JobOutcome::Failed(TerminalOutcome::InfraErr {
                    stage: "probe_disconnect".into(),
                    message: "probe disconnected mid-run".into(),
                });
            }
            Ok(None) => {
                // No event this tick; loop back to watchdog check.
                continue;
            }
            Err(e) => {
                return JobOutcome::Failed(TerminalOutcome::InfraErr {
                    stage: "probe_io".into(),
                    message: format!("{e}"),
                });
            }
        }
    }
}

/// Convert a watchdog stop reason into a terminal `JobOutcome`.
///
/// TODO(M6.4): when the real probe session lands, this function should
/// pulse `Cancel` into the probe via `_session`, wait up to `_release_grace`
/// for the probe to drop, and mark `probe_unresponsive: true` on the
/// returned outcome if the grace expires. Today both parameters are
/// reserved as a forward-compat seam — do not drop them in cleanup.
fn finalise_for_stop(
    reason: StopReason,
    started_at: Instant,
    _release_grace: Duration,
    _session: &mut Box<dyn ProbeSession>,
) -> JobOutcome {
    let elapsed_ms =
        u64::try_from(Instant::now().duration_since(started_at).as_millis()).unwrap_or(u64::MAX);
    match reason {
        StopReason::Inactivity => JobOutcome::TimedOut {
            reason: TimeoutReason::Inactivity,
            elapsed_ms,
        },
        StopReason::HardMax => JobOutcome::TimedOut {
            reason: TimeoutReason::HardMax,
            elapsed_ms,
        },
        StopReason::UserCancel => JobOutcome::Aborted {
            by: AbortReason::User,
        },
        StopReason::DaemonShutdown => JobOutcome::Aborted {
            by: AbortReason::DaemonShutdown,
        },
    }
}
