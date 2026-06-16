//! Job state machine, priority, source, and outcome types.

use crate::board::BoardSelector;
use crate::ids::JobId;
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
    /// paavod restarted while this job was still building/running; the
    /// startup reconciliation pass terminalized the orphaned row.
    Interrupted,
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
/// HTTP submit metadata. This is the JSON shape paavo-cli (and any
/// other HTTP client) sends as the `metadata` part of `POST /jobs`.
/// paavod deserializes it with `#[serde(deny_unknown_fields)]`, so:
/// - `source` is NOT here — every HTTP submit is recorded as
///   `JobSource::Cli` (the scheduler reaches `paavo_core::enqueue_job`
///   directly without going through HTTP).
/// - `tar_blake3` is NOT here — paavod computes it from the uploaded
///   tar bytes during streaming.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSpec {
    /// Scheduler priority.
    pub priority: Priority,
    /// Free-form submitter id (no auth).
    pub submitter: String,
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
    /// Skip the build cache for this job. When `false` (default),
    /// paavod consults `build_cache` keyed by `tar_blake3` — an
    /// identical resubmit reuses the prior ELF. When `true`, paavod
    /// always invokes `cargo build --release` and flashes the freshly-
    /// produced binary. Used by `paavo-cli run --skip-cache` when
    /// chasing a flaky chip or a transient toolchain issue where the
    /// operator wants a clean build path. The cache row, if one
    /// exists, is left alone (not invalidated) so subsequent
    /// without-`--skip-cache` submits still hit it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_cache: bool,
}

/// JSON shape returned by `GET /jobs` and `GET /jobs/:id`. Mirrors the
/// internal `paavo_db::JobRow` but **excludes server-local filesystem
/// paths** (`tar_path`, `elf_path`) so the daemon's state-directory
/// layout is not leaked to HTTP clients. `tar_blake3` IS exposed
/// because it is content-addressed and useful for operators debugging
/// the build cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobView {
    /// Job id.
    pub id: JobId,
    /// Scheduler priority at submission time (may have been promoted
    /// since via the starvation threshold).
    pub priority: Priority,
    /// Free-form submitter id.
    pub submitter: String,
    /// Where the job came from.
    pub source: JobSource,
    /// Board selector.
    pub board_selector: BoardSelector,
    /// Effective inactivity timeout (ms).
    pub inactivity_timeout_ms: u64,
    /// Hard-max wall clock (ms).
    pub hard_max_ms: u64,
    /// Current persistent state.
    pub state: JobState,
    /// Decoded outcome when terminal, else `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<JobOutcome>,
    /// Board the job was dispatched to (set on Building/Running and
    /// after).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_id: Option<String>,
    /// Submission time, epoch ms.
    pub submitted_at: i64,
    /// Time the scheduler picked the job, epoch ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// Time the worker reached terminal state, epoch ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// blake3 of the uploaded crate tar (content-addressed; useful for
    /// build-cache debugging).
    pub tar_blake3: String,
    /// Packages threaded into `paavo_build::BuildPlan::cargo_update_packages`.
    /// Empty for HTTP-submitted jobs; populated for Scheduled jobs from
    /// `[[corpus]].cargo_update`. Exposed on the wire so paavo-cli/web
    /// can show "this nightly job will pull fresh embassy revisions".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cargo_update_packages: Vec<String>,
    /// `true` if the job was submitted with `--skip-cache`. Surfaced
    /// for paavo-cli/web so operators can see why an otherwise
    /// cacheable resubmit took the slow path.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_cache: bool,
}
