//! Sandbox tar unpack, `cargo build`, and ELF discovery for paavo.
//!
//! ```
//! assert_eq!(paavo_build::CRATE_NAME, "paavo-build");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-build";
