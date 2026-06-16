//! Abstraction over `paavo-runner::run_job`. Production code wires this to
//! the real BoardWorker. Tests substitute a deterministic in-process impl.

use paavo_proto::JobOutcome;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

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

/// Per-job context handed to `Runner::run`. Carries the job/board ids plus
/// the shared log-frame seq counter and job-start clock so the run-phase
/// log forwarder numbers and timestamps its frames continuously with the
/// build-phase forwarder (both restamp from this shared state). See
/// `docs/superpowers/specs/2026-06-16-c2-log-frame-persistence-design.md`.
pub struct RunContext<'a> {
    /// Job being run.
    pub job_id: paavo_proto::JobId,
    /// Board the job was dispatched to.
    pub board_id: &'a str,
    /// Shared per-job log-frame seq counter (created in dispatch, also
    /// handed to the build forwarder). `fetch_add(1, Relaxed)` per frame.
    pub log_seq: Arc<AtomicU64>,
    /// Monotonic job-execution start. `ts_us` is microseconds since this.
    pub job_start: Instant,
}

/// Production code passes `Arc<dyn Runner>`; tests pass `FakeRunner`.
pub trait Runner: Send + Sync {
    /// Run a job on `ctx.board_id` and block until terminal. The job has
    /// already had its row transitioned to `Building` and its tar/ELF
    /// resolved by the caller.
    fn run(&self, ctx: RunContext<'_>) -> RunOutcome;
}
