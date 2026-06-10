//! Abstraction over `paavo-runner::run_job`. Production code wires this to
//! the real BoardWorker. Tests substitute a deterministic in-process impl.

use paavo_proto::JobOutcome;

/// What a runner reports back when a job finishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// Terminal job outcome.
    pub outcome: JobOutcome,
    /// True if the BoardWorker successfully released the probe before the
    /// release-grace expired. Per spec §5.2, when this is `false` and the
    /// outcome is `TimedOut{Inactivity}`, the board's infra-failure counter
    /// must be bumped.
    pub probe_released_cleanly: bool,
}

/// Production code passes `Box<dyn Runner>`; tests pass `FakeRunner`.
pub trait Runner: Send + Sync {
    /// Run a job on `board_id` and block until terminal. The job has
    /// already had its row transitioned to `Building` and its tar/ELF
    /// resolved by the caller.
    fn run(&self, job_id: paavo_proto::JobId, board_id: &str) -> RunOutcome;
}
