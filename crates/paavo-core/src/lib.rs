//! Scheduler, board fleet, and quarantine policy for paavo. No HTTP lives
//! in this crate; see `paavod` for the axum surface.
//!
//! ```
//! assert_eq!(paavo_core::CRATE_NAME, "paavo-core");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-core";
