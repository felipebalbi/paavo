//! Per-job runner: owns one probe via `paavo-probe`, runs the inactivity
//! and hard-max watchdog, streams `LogFrame` events out, and returns a
//! terminal `JobOutcome`.
//!
//! ```
//! assert_eq!(paavo_runner::CRATE_NAME, "paavo-runner");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-runner";

mod job;
mod watchdog;
mod worker;

pub use job::{JobInputs, JobOutputs, RunCommand};
pub use worker::{run_job, BoardWorkerHandle};
