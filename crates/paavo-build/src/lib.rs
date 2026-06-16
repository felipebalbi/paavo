//! Tar unpack, `cargo build`, and ELF discovery for paavo. Build-cache
//! plumbing (paired with `paavo-db::BuildCacheEntry`) lives in
//! `paavo-core::build_cache` — this crate stays free of any DB dep so spec
//! §4.1's boundary ("paavo-build depends only on paavo-proto") holds.
//!
//! ```
//! assert_eq!(paavo_build::CRATE_NAME, "paavo-build");
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Crate name, used by a smoke doctest.
pub const CRATE_NAME: &str = "paavo-build";

mod build;
mod elf;
mod error;
pub mod tar;

pub use build::{
    build_release, build_release_streaming, build_release_streaming_cancellable, BuildLine,
    BuildLineTx, BuildPlan, BuildResult, BuildStream,
};
pub use elf::{discover_elf, ManifestArtifactHint};
pub use error::{BuildError, Result};
