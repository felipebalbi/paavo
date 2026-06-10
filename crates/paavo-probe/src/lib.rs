//! Low-level probe driver. Wraps `probe-rs` and `defmt-decoder` and parses
//! the `.teleprobe.*` ELF sections that scaffolded test crates emit via the
//! `paavo-meta` macros.
//!
//! Layered for testability:
//! - [`sections`] — pure ELF byte parser (no probe-rs).
//! - [`Event`] — variants streamed back to `paavo-runner`.
//! - [`ProbeSession`] — the probe-rs adapter surface. Real impl wraps
//!   `probe_rs::Session`; a mock impl lives in `paavo-runner` tests so the
//!   BoardWorker is driven deterministically without hardware.
//!
//! ```
//! assert_eq!(paavo_probe::CRATE_NAME, "paavo-probe");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-probe";

mod error;
mod event;
pub mod sections;
mod session;

pub use error::{ProbeError, Result};
pub use event::Event;
pub use session::{ProbeSession, RealSession, RealSessionOptions};
