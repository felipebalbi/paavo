//! Probe session abstraction. The real `probe-rs` + `defmt-decoder` adapter
//! lives behind this trait; tests in `paavo-runner` (and elsewhere) stub it
//! with a deterministic mock.
//!
//! The real adapter is **stubbed** in this milestone (M2.1). The full
//! `probe-rs` wiring lands in Milestone 6.4 (hardware smoke), when there
//! is a physical probe and board available to validate it against.
//! `RealSession::connect` and `RealSession::next_event` both error
//! immediately today; only the trait shape and option struct are stable.

use crate::error::{ProbeError, Result};
use crate::event::Event;

/// Long-lived probe session that flashes and observes a single test.
///
/// Implementors must be `Send` because the BoardWorker thread owns the
/// session for the duration of a job. `Sync` is deliberately NOT required:
/// the real `probe-rs` session is single-threaded, and M6.4's `RealSession`
/// will (transitively) not implement `Sync` once it holds a `probe_rs::Session`.
/// Mock sessions used in `paavo-runner`'s tests are likewise free to be
/// `!Sync`.
pub trait ProbeSession: Send {
    /// Block until the next event is available (up to `timeout_ms`
    /// milliseconds), or return `Ok(None)` if the target has reached a
    /// clean stop. Implementations may return events back-to-back with no
    /// inter-event delay.
    fn next_event(&mut self, timeout_ms: u32) -> Result<Option<Event>>;
}

/// Connection options for the real probe-rs adapter.
#[derive(Debug, Clone)]
pub struct RealSessionOptions {
    /// USB selector for probe-rs.
    pub probe_selector: paavo_proto::ProbeSelector,
    /// probe-rs chip name.
    pub chip_name: String,
    /// Path to the ELF to flash and run.
    pub elf_path: std::path::PathBuf,
    /// If true, skip the post-load reset (NXP RT685S quirk; see spec §2).
    pub skip_post_load_reset: bool,
}

/// Real `probe-rs` + `defmt-decoder` backed session. Fully wired in
/// Milestone 6.4 (hardware smoke); the in-tree tests use a mock session.
pub struct RealSession {
    // Stored for the future implementation in M6.4; field is intentionally
    // unused today.
    #[allow(dead_code)]
    opts: RealSessionOptions,
}

impl RealSession {
    /// Connect to a probe, flash the ELF, and start RTT.
    ///
    /// **Hardware-only** — this constructor reaches out to probe-rs and
    /// requires a physical probe + board. Workspace tests must use a mock
    /// impl of `ProbeSession`.
    ///
    /// In this milestone the body is an explicit stub that always returns
    /// an error mentioning M6.4. The signature is stable so callers can
    /// compile against it today; the implementation lands together with
    /// the probe-rs wiring in Task 6.4.
    pub fn connect(_opts: RealSessionOptions) -> Result<Self> {
        Err(ProbeError::ProbeRs(
            "RealSession::connect is wired in Milestone 6.4; \
             use a mock ProbeSession for in-workspace tests"
                .into(),
        ))
    }
}

impl ProbeSession for RealSession {
    fn next_event(&mut self, _timeout_ms: u32) -> Result<Option<Event>> {
        Err(ProbeError::ProbeRs(
            "RealSession::next_event is wired in Milestone 6.4".into(),
        ))
    }
}
