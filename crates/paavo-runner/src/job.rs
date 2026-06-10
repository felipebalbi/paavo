//! Per-job input / output shapes for `run_job`.

use crossbeam_channel::{Receiver, Sender};
use paavo_proto::{JobId, LogFrame};

/// Inputs the caller provides to a BoardWorker.
pub struct JobInputs {
    /// Job being executed.
    pub job_id: JobId,
    /// Effective inactivity timeout for this job, in **milliseconds**.
    pub inactivity_timeout_ms: u64,
    /// Effective hard-max wall clock for this job, in **milliseconds**.
    pub hard_max_ms: u64,
    /// How long to wait, after we ask the worker to stop, before declaring
    /// the probe unresponsive and counting an infra failure. Wired in M6.4
    /// alongside the real probe-rs session; currently reserved for future use.
    pub probe_release_grace_ms: u64,
    /// Cancel signal channel — receive end is checked by the watchdog.
    pub cancel_rx: Receiver<RunCommand>,
}

/// Outputs produced by a BoardWorker.
pub struct JobOutputs {
    /// LogFrame stream — closed when the worker exits.
    pub log_tx: Sender<LogFrame>,
}

/// External commands to a running job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunCommand {
    /// User-requested cancel. Watchdog signals worker; on timely release,
    /// outcome is `Aborted{User}`.
    Cancel,
    /// Daemon shutdown drain. Same as `Cancel` but produces
    /// `Aborted{DaemonShutdown}`.
    DaemonShutdown,
}
