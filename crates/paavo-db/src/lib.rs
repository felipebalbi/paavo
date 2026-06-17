//! SQLite-backed persistence for paavo. Owns the schema; exposes typed
//! query helpers per table. Single writer (paavod), single reader
//! (paavo-web).
//!
//! ```
//! assert_eq!(paavo_db::CRATE_NAME, "paavo-db");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-db";

mod board;
mod build_cache;
mod db;
mod error;
mod job;
mod like;
mod log;
mod schedule;

pub use board::BoardRow;
pub use build_cache::{BuildCacheEntry, BuildCacheStats};
pub use db::Db;
pub use error::{DbError, Result};
pub use job::{JobRow, NewJob, OutcomeRecord};
pub use log::{LogFrameDb, LogFrameRow};
pub use schedule::{ScheduleRow, ScheduleUpdate};
