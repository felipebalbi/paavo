//! Job state machine, priority, source, and outcome types.

use crate::board::BoardSelector;
use serde::{Deserialize, Serialize};

/// Scheduler priority. Lower variant value = higher priority. Serializes as
/// snake_case strings on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Ad-hoc developer requests (`paavo-cli run`).
    Interactive,
    /// Nightly cron jobs.
    Scheduled,
}

impl Priority {
    /// Numeric weight used by the scheduler's `BinaryHeap` AND persisted as
    /// the SQL `job.priority` column (per spec §7.2). Smaller = sooner.
    pub fn weight(self) -> u8 {
        match self {
            Priority::Interactive => 0,
            Priority::Scheduled => 1,
        }
    }
}

/// Where a job came from. Distinct from `Priority` because a starvation-
/// promoted Scheduled job retains `JobSource::Scheduler`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobSource {
    /// Submitted via `paavo-cli`.
    Cli,
    /// Submitted by the nightly scheduler.
    Scheduler,
}

/// One of the seven persistent states in the job state machine. See
/// `JobOutcome` for the finer-grained terminal-state information.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    /// Accepted by the daemon, not yet dispatched.
    #[serde(rename = "submitted")]
    Submitted,
    /// `paavo-build` is compiling.
    #[serde(rename = "building")]
    Building,
    /// `paavo-runner` is attached to a probe.
    #[serde(rename = "running")]
    Running,
    /// Terminal: test reported `Test OK` + bkpt.
    #[serde(rename = "passed")]
    Passed,
    /// Terminal: test failed (build error, test error, or infra error).
    #[serde(rename = "failed")]
    Failed,
    /// Terminal: inactivity or hard-max watchdog tripped. Wire form is
    /// `"timedout"` (one word), matching the SQL CHECK constraint.
    #[serde(rename = "timedout")]
    TimedOut,
    /// Terminal: user cancel or daemon shutdown.
    #[serde(rename = "aborted")]
    Aborted,
}

impl JobState {
    /// True for `Passed`/`Failed`/`TimedOut`/`Aborted`.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Passed | JobState::Failed | JobState::TimedOut | JobState::Aborted
        )
    }
}

/// Specific reason for a `Failed` terminal state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalOutcome {
    /// `cargo build` failed.
    BuildErr {
        /// Captured stderr from cargo (truncated by daemon if huge).
        stderr: String,
    },
    /// Test ran but failed: panic, assert, or defmt-encoded error frame.
    TestErr {
        /// Human-readable summary.
        message: String,
    },
    /// Infrastructure failure: probe attach, mass erase, RTT init, etc.
    /// Contributes to consecutive-infra-failure quarantine count.
    InfraErr {
        /// Pipeline stage that failed (`probe_attach`, `flash`, `rtt_init`,
        /// `defmt_decode`, ...).
        stage: String,
        /// Underlying error.
        message: String,
    },
}

/// Why a job timed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutReason {
    /// No defmt frame for `inactivity_timeout` seconds.
    Inactivity,
    /// Total wall clock exceeded `hard_max`.
    HardMax,
}

/// Who initiated an abort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortReason {
    /// `paavo-cli cancel`.
    User,
    /// SIGTERM drain ran out of grace.
    DaemonShutdown,
}

/// Fully-tagged terminal outcome stored in the `job.outcome_detail` JSON
/// column and returned on the wire.
///
/// Wire format (externally-tagged outer, internally-tagged inner):
/// - `Passed` → `"passed"`                 (bare string, not an object)
/// - `Failed(TerminalOutcome::TestErr { message })`  → `{"failed":{"kind":"test_err","message":"..."}}`
/// - `Failed(TerminalOutcome::BuildErr { stderr })`  → `{"failed":{"kind":"build_err","stderr":"..."}}`
/// - `Failed(TerminalOutcome::InfraErr { stage, message })` → `{"failed":{"kind":"infra_err","stage":"...","message":"..."}}`
/// - `TimedOut { reason, elapsed_ms }`     → `{"timed_out":{"reason":"inactivity","elapsed_ms":120000}}`
/// - `Aborted { by }`                      → `{"aborted":{"by":"user"}}`
///
/// **Consumers must inspect both layers** — `"failed"` alone does not tell
/// you it's an infrastructure failure; the inner `kind` tag does. See also
/// [`JobOutcome::counts_toward_infra_failure`].
///
/// Externally-tagged is used (instead of `#[serde(tag = "outcome")]`)
/// because internal tagging does not support tuple variants like
/// `Failed(TerminalOutcome)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOutcome {
    /// Reached `Test OK` + bkpt.
    Passed,
    /// Failure with detail.
    Failed(TerminalOutcome),
    /// Watchdog fired.
    TimedOut {
        /// Cause.
        reason: TimeoutReason,
        /// Elapsed time at the moment the watchdog fired.
        elapsed_ms: u64,
    },
    /// Aborted with detail.
    Aborted {
        /// Who.
        by: AbortReason,
    },
}

impl JobOutcome {
    /// True if the outcome should bump the board's consecutive_infra_failures
    /// counter. Per spec §5.2:
    /// - `Failed(InfraErr)` → yes
    /// - other outcomes → no (caller may additionally count
    ///   `TimedOut(Inactivity)` only when the BoardWorker could not release
    ///   the probe; that knowledge does not live in `JobOutcome` itself).
    pub fn counts_toward_infra_failure(&self) -> bool {
        matches!(self, JobOutcome::Failed(TerminalOutcome::InfraErr { .. }))
    }
}

/// The request side of a job, as serialised in the `POST /jobs` multipart
/// JSON part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSpec {
    /// Scheduler priority.
    pub priority: Priority,
    /// Free-form submitter id (no auth).
    pub submitter: String,
    /// Where the request came from.
    pub source: JobSource,
    /// Board match rules.
    pub board_selector: BoardSelector,
    /// Per-job inactivity override. `None` means use the ELF's
    /// `inactivity_timeout!()`, falling back to the daemon default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inactivity_timeout_ms: Option<u64>,
    /// Per-job hard-max override. `None` means use the daemon default for
    /// the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hard_max_ms: Option<u64>,
    /// blake3 of the uploaded crate tar, used as build-cache key.
    pub tar_blake3: String,
}
