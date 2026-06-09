//! Low-level probe driver. Wraps `probe-rs` and `defmt-decoder` and parses
//! `.teleprobe.*` ELF sections.
//!
//! ```
//! assert_eq!(paavo_probe::CRATE_NAME, "paavo-probe");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-probe";
