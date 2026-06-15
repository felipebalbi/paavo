//! Real probe-rs-backed runner. Replaces the M2.1 unit-struct stub.
//!
//! Owns the four pieces of state needed to drive a real BoardWorker:
//! - DB handle: read `elf_path` + the board's `BoardSpec` per job.
//! - JobLogsBroker: stream decoded defmt frames to live tail listeners.
//! - CancellationRegistry: consume the rx half via `take_receiver` so
//!   the watchdog can read user/daemon cancel signals directly.
//! - Config: timeouts (per-job inactivity/hard-max ride on the JobRow;
//!   probe_release_grace is a literal for v1 — see TODO below).
//!
//! M7 sub-tasks:
//!   7.3 - skeleton: `run()` read elf_path + returned InfraErr.
//!   7.4 - `RealSession::connect` landed.
//!   7.5 - `RealSession::next_event` landed (RTT + defmt + bkpt).
//!   7.6 - this step. Stitches `paavo_runner::run_job` to RealSession,
//!         pumps log frames into the live broker, and returns real
//!         outcomes. This is the M7 demo path.

use crate::app_state::AppState;
use crate::cancellation::CancellationRegistry;
use crate::config::Config;
use crate::job_logs::{JobLogsBroker, LiveEvent};
use paavo_core::{RunOutcome, Runner};
use paavo_db::{BoardRow, Db, JobRow};
use paavo_proto::{JobId, JobOutcome, TerminalOutcome};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

/// Probe-release grace in ms — how long the runner waits after a
/// stop signal for the probe to drop before declaring it
/// unresponsive (M8 wires the actual `Cancel`-pulse-into-probe
/// behaviour; today the value flows through to `run_job` and is
/// reserved on `WatchdogState`).
///
/// TODO(spec §6.2 + config schema follow-up): make this a config
/// knob. The M4.1 `TimeoutsConfig` schema doesn't yet expose it —
/// adding it now would touch every paavo.toml in the wild, and the
/// value is dead code until M8 anyway. A literal of 2_000 (2 s)
/// matches what the spec proposes as the v2 default.
const PROBE_RELEASE_GRACE_MS: u64 = 2_000;

/// Real probe-rs-backed runner.
///
/// Constructed once at daemon startup (see `paavod::main`) and shared
/// across every dispatch via `Arc<dyn Runner>`. Stitches
/// `paavo_runner::run_job` + `paavo_probe::RealSession` together,
/// pumps decoded `LogFrame`s into the live `JobLogsBroker`, and
/// returns the resulting `JobOutcome`.
pub struct RealRunner {
    db: Arc<Mutex<Db>>,
    job_logs: JobLogsBroker,
    cancellation: CancellationRegistry,
    #[allow(dead_code)] // reserved for M8 (probe defaults, retention rules)
    config: Arc<Config>,
}

impl RealRunner {
    /// Construct a `RealRunner` from its four pieces of state. The
    /// `Arc`/`Clone` wrappers mean callers can share state with the
    /// rest of the daemon without extra plumbing.
    pub fn new(
        db: Arc<Mutex<Db>>,
        job_logs: JobLogsBroker,
        cancellation: CancellationRegistry,
        config: Arc<Config>,
    ) -> Self {
        Self {
            db,
            job_logs,
            cancellation,
            config,
        }
    }

    /// Construct from `AppState`. The state pieces are all `Clone`
    /// (Arc-wrapped), so no extra plumbing is needed in `main.rs`.
    pub fn from_state(state: &AppState) -> Self {
        Self::new(
            state.db.clone(),
            state.job_logs.clone(),
            state.cancellation.clone(),
            state.config.clone(),
        )
    }
}

impl Runner for RealRunner {
    fn run(&self, job_id: JobId, board_id: &str) -> RunOutcome {
        // 1. Read elf_path + board spec + per-job timeouts under one
        //    DB lock. Lock duration is bounded (two indexed SELECTs).
        let lookup = {
            let db = self.db.lock();
            let job = match JobRow::find(db.raw_conn(), &job_id) {
                Ok(Some(j)) => j,
                Ok(None) => {
                    return infraerr(
                        "real_runner.db_lookup",
                        format!("job row not found: {job_id}"),
                    );
                }
                Err(e) => {
                    return infraerr(
                        "real_runner.db_lookup",
                        format!("DB error reading job row: {e}"),
                    );
                }
            };
            let board = match BoardRow::find(db.raw_conn(), board_id) {
                Ok(Some(b)) => b,
                Ok(None) => {
                    return infraerr(
                        "real_runner.db_lookup",
                        format!("board row not found: {board_id}"),
                    );
                }
                Err(e) => {
                    return infraerr(
                        "real_runner.db_lookup",
                        format!("DB error reading board row: {e}"),
                    );
                }
            };
            (job, board.spec)
        };
        let (job, board_spec) = lookup;

        // `elf_path` is `Option<String>` at the schema level. Dispatch
        // always calls `transition_to_running(elf_path)` before
        // `runner.run`, so a NULL here means dispatch's contract was
        // violated — surface as InfraErr rather than panicking.
        let elf_path = match job.elf_path {
            Some(p) => PathBuf::from(p),
            None => {
                return infraerr(
                    "real_runner.elf_path_missing",
                    format!(
                        "elf_path column is NULL on job row {job_id} — \
                         dispatch did not transition past Building before \
                         invoking runner.run"
                    ),
                );
            }
        };

        // 2. Take the cancel rx out of the registry. Dispatch's
        //    `register(job_id)` allocated the channel; the sender
        //    stays in the registry so a `POST /jobs/:id/cancel` keeps
        //    routing.
        //
        //    If `take_receiver` returns `None`, the dispatch contract
        //    was violated (`register` was never called for this job)
        //    OR the rx was already taken by an earlier `runner.run`
        //    call. EITHER way the cancel path is dead: a fallback to
        //    a disconnected channel would let the run "succeed" but
        //    `POST /jobs/:id/cancel` would silently return 204 while
        //    the watchdog never sees the Cancel. Surface as InfraErr
        //    instead — symmetry with the `elf_path == None` branch
        //    above. Reviewers flagged this in M7.6's quality review.
        let cancel_rx = match self.cancellation.take_receiver(&job_id) {
            Some(rx) => rx,
            None => {
                return infraerr(
                    "real_runner.cancel_registry_missing",
                    format!(
                        "no cancel-channel entry for job {job_id} — \
                         dispatch did not call CancellationRegistry::register \
                         before runner.run, or another runner already took \
                         the receiver. Cancel path would be dead; refusing \
                         to run."
                    ),
                );
            }
        };

        // 3. Build JobInputs + JobOutputs.
        //
        //    Per-job timeouts come off the JobRow (HTTP/CLI overrides
        //    were validated at enqueue against `daemon_ceiling_s`).
        //    Falling back to config defaults here would silently
        //    override caller intent — spec §6.2 wants per-job values.
        let (log_tx, log_rx) = crossbeam_channel::unbounded();
        let inputs = paavo_runner::JobInputs {
            job_id,
            inactivity_timeout_ms: job.inactivity_timeout_ms,
            hard_max_ms: job.hard_max_ms,
            probe_release_grace_ms: PROBE_RELEASE_GRACE_MS,
            cancel_rx,
        };
        let outputs = paavo_runner::JobOutputs { log_tx };

        // 4. Spawn forwarder thread: drain log_rx → JobLogsBroker.
        //    The thread exits when the worker drops log_tx (which
        //    happens when run_job's worker thread returns).
        let broker = self.job_logs.clone();
        let fwd = std::thread::Builder::new()
            .name("paavod-log-forwarder".into())
            .spawn(move || {
                while let Ok(frame) = log_rx.recv() {
                    broker.publish(job_id, LiveEvent::Frame(frame));
                }
            })
            .expect("spawn log forwarder thread");

        // 5. Build the make_session closure. RealSession::connect is
        //    fallible; the runner's worker handles the error path via
        //    JobOutcome::Failed(InfraErr { stage: "probe_attach" }).
        //
        //    skip_post_load_reset is a literal false: BoardSpec
        //    doesn't carry it (the RT685S quirk is deferred to M8 per
        //    spec §17 "Deferred from M7"). When M8 lands, add a field
        //    to BoardSpec and thread it through here.
        let opts = paavo_probe::RealSessionOptions {
            probe_selector: board_spec.probe_selector.clone(),
            chip_name: board_spec.chip_name.clone(),
            elf_path,
            skip_post_load_reset: false,
        };
        let make_session = move || -> paavo_probe::Result<Box<dyn paavo_probe::ProbeSession>> {
            let s = paavo_probe::RealSession::connect(opts)?;
            Ok(Box::new(s) as Box<dyn paavo_probe::ProbeSession>)
        };

        // 6. Run the worker to completion, join the forwarder.
        let handle = paavo_runner::run_job(inputs, outputs, make_session);
        let outcome = handle.join();
        // Forwarder exits when log_tx is dropped (worker thread
        // exited inside `handle.join()`). Joining here ensures the
        // last live frames have been published before we publish
        // the terminal event in dispatch. If the forwarder panicked
        // (broker bug, OOM-during-publish, etc.), surface it via
        // tracing — silently swallowing was masking real failures.
        if let Err(payload) = fwd.join() {
            let msg = panic_message(&payload);
            tracing::error!(%job_id, msg, "log forwarder thread panicked");
        }

        // probe_released_cleanly is hard-coded `true` for v1:
        // detecting unresponsive-probe-on-stop is M8 work (see
        // `paavo_runner::worker::finalise_for_stop` TODO and spec
        // §17). The watchdog grace window is wired into JobInputs
        // but not yet read on the path back out.
        RunOutcome {
            outcome,
            probe_released_cleanly: true,
        }
    }
}

/// Build an `InfraErr` outcome with `probe_released_cleanly: true`.
/// Used by every early-return path in `Runner::run` before the probe
/// session is constructed.
fn infraerr(stage: &str, message: String) -> RunOutcome {
    RunOutcome {
        outcome: JobOutcome::Failed(TerminalOutcome::InfraErr {
            stage: stage.into(),
            message,
        }),
        probe_released_cleanly: true,
    }
}

/// Extract a human-readable message from a panic payload caught via
/// `thread::JoinHandle::join`. Mirrors `dispatch::panic_message` so
/// the two callers produce identical operator-facing strings.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}
