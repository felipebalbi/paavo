//! Real probe-rs-backed runner. Replaces the M2.1 unit-struct stub.
//!
//! Owns the four pieces of state needed to drive a real BoardWorker:
//! - DB handle: read `elf_path` from the job row (dispatch already
//!   wrote it via `transition_to_running`).
//! - JobLogsBroker: stream decoded defmt frames to live tail listeners.
//! - CancellationRegistry: read the cancel rx for the job (M7 stub;
//!   M8 acts on it inside the inner loop).
//! - Config: timeouts (hard_max, inactivity, probe_release_grace) and
//!   probe defaults.
//!
//! M7 sub-tasks:
//!   7.3 - this skeleton (current step). `run()` reads the elf_path,
//!         returns InfraErr citing it. No probe-rs calls yet.
//!   7.4 - `RealSession::connect` lands; `run()` calls it and returns
//!         InfraErr citing connect failure (no run loop yet).
//!   7.6 - stitches `paavo_runner::run_job` and returns real outcomes.

use crate::app_state::AppState;
use crate::cancellation::CancellationRegistry;
use crate::config::Config;
use crate::job_logs::JobLogsBroker;
use paavo_core::{RunOutcome, Runner};
use paavo_db::{Db, JobRow};
use paavo_proto::{JobId, JobOutcome, TerminalOutcome};
use parking_lot::Mutex;
use std::sync::Arc;

/// Real probe-rs-backed runner skeleton.
///
/// Constructed once at daemon startup (see `paavod::main`) and shared
/// across every dispatch via `Arc<dyn Runner>`. The actual probe-rs +
/// `paavo_runner::run_job` wiring lands in M7.4-7.6; until then `run`
/// reads `elf_path` from the job row and returns an `InfraErr` that
/// names exactly what would have been flashed.
pub struct RealRunner {
    db: Arc<Mutex<Db>>,
    #[allow(dead_code)] // wired in 7.6
    job_logs: JobLogsBroker,
    #[allow(dead_code)] // wired in 7.6 (read rx); used by M8 to abort
    cancellation: CancellationRegistry,
    #[allow(dead_code)] // wired in 7.4 (probe defaults) + 7.6 (timeouts)
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
    fn run(&self, job_id: JobId, _board_id: &str) -> RunOutcome {
        // Read elf_path off the job row. Dispatch already wrote it via
        // `transition_to_running`. If the row is missing (a race we do
        // not expect, but be defensive), report InfraErr with
        // stage=real_runner.db_lookup so the operator sees a clear,
        // structured error pointing at the lookup.
        let elf_path = {
            let db = self.db.lock();
            match JobRow::find(db.raw_conn(), &job_id) {
                Ok(Some(row)) => row.elf_path,
                Ok(None) => {
                    return RunOutcome {
                        outcome: JobOutcome::Failed(TerminalOutcome::InfraErr {
                            stage: "real_runner.db_lookup".into(),
                            message: format!("job row not found: {job_id}"),
                        }),
                        probe_released_cleanly: true,
                    };
                }
                Err(e) => {
                    return RunOutcome {
                        outcome: JobOutcome::Failed(TerminalOutcome::InfraErr {
                            stage: "real_runner.db_lookup".into(),
                            message: format!("DB error reading job row: {e}"),
                        }),
                        probe_released_cleanly: true,
                    };
                }
            }
        };

        // `elf_path` is `Option<String>`. NULL means dispatch did not
        // transition the row past Building before invoking us — not
        // expected under the current dispatch contract (which always
        // calls `transition_to_running(elf_path)` before `runner.run`),
        // but the column is `Option` at the schema level so we surface
        // a clear placeholder rather than panicking. We reuse the
        // `real_session.connect_unwired` stage here (rather than
        // inventing a third stage that no test exercises) so operators
        // see one consistent shape regardless of the NULL-vs-Some path.
        let elf_display = elf_path
            .as_deref()
            .unwrap_or("(elf_path column was NULL on job row)");

        // M7 sub-tasks 7.4-7.6 replace this with: RealSession::connect →
        // paavo_runner::run_job → outcome mapping. (7.5 lives entirely
        // inside RealSession and does not touch this run() body — it
        // implements `ProbeSession::next_event` for the real adapter.)
        // Until then we return an InfraErr that names the resolved
        // elf_path so operators see exactly what would have been
        // flashed and can sanity-check build_or_cache before the
        // runner lands.
        RunOutcome {
            outcome: JobOutcome::Failed(TerminalOutcome::InfraErr {
                stage: "real_session.connect_unwired".into(),
                message: format!(
                    "RealSession is wired in Milestone 7.4. \
                     Would have flashed: {elf_display}"
                ),
            }),
            probe_released_cleanly: true,
        }
    }
}
