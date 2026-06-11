//! paavod library — pulled out so integration tests can `use paavod::*`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name.
pub const CRATE_NAME: &str = "paavod";

pub mod config;
pub mod state_dir;
