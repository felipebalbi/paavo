//! Per-job runner: owns one probe via paavo-probe, runs the inactivity
//! and hard-max watchdog, emits LogFrame events.
//!
//! ```
//! assert_eq!(paavo_runner::CRATE_NAME, "paavo-runner");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-runner";
