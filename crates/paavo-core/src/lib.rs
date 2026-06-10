//! Scheduler, board fleet, and quarantine policy for paavo. No HTTP — the
//! `paavod` crate owns axum and wraps a `Core` handle.
//!
//! ```
//! assert_eq!(paavo_core::CRATE_NAME, "paavo-core");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-core";

mod enqueue;
mod error;
mod quarantine;
mod runner;
mod scheduler;
mod selector;

pub use enqueue::{enqueue_job, EnqueueRequest};
pub use error::{CoreError, Result};
pub use quarantine::{apply_outcome_to_board, auto_quarantine_reason, QuarantinePolicy};
pub use runner::{RunOutcome, Runner};
pub use scheduler::{pick_next, ScheduledJob, SchedulerConfig};
pub use selector::selector_matches_any;
