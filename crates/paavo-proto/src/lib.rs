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
