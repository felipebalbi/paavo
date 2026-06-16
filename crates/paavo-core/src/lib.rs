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

mod build_cache;
mod cancel;
mod enqueue;
mod error;
mod quarantine;
mod runner;
mod scheduler;
mod selector;

pub use build_cache::{cache_lookup, cache_store, evict_lru, CacheLookup};
pub use cancel::cancel_if_submitted;
pub use enqueue::{enqueue_job, validate_enqueue, EnqueueRequest};
pub use error::{CoreError, Result};
pub use quarantine::{apply_outcome_to_board, auto_quarantine_reason, QuarantinePolicy};
pub use runner::{RunContext, RunOutcome, Runner};
pub use scheduler::{pick_next, ScheduledJob, SchedulerConfig};
pub use selector::selector_matches_any;
