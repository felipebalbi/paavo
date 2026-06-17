//! Wire types and protocol definitions for paavo.
//!
//! This crate has no workspace dependencies. It is pure data: every other
//! paavo crate is permitted to depend on `paavo-proto`, and `paavo-proto`
//! depends on none of them.
//!
//! ```
//! assert_eq!(paavo_proto::CRATE_NAME, "paavo-proto");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-proto";

mod board;
mod ids;
mod job;
mod log;
mod page;
mod schedule;
mod stats;
mod stream;

pub use board::{BoardHealth, BoardSelector, BoardSpec, BoardView, ProbeSelector};
pub use ids::JobId;
pub use job::{
    AbortReason, JobListItem, JobOutcome, JobSource, JobSpec, JobState, JobView, Priority,
    TerminalOutcome, TimeoutReason,
};
pub use log::{LogFrame, LogLevel};
pub use page::Page;
pub use schedule::ScheduleView;
pub use stats::{BoardHealthCounts, DashboardOverview, JobStateCounts};
pub use stream::{JobPhase, WireMessage};
