//! Errors returned by paavo-build operations.

use thiserror::Error;

/// Errors from tar unpack, cargo invocation, and ELF discovery.
#[derive(Debug, Error)]
pub enum BuildError {
    /// An entry inside the archive had a path that would escape the
    /// destination directory (absolute path or contained `..`).
    #[error("path-escape: entry {path:?} would escape sandbox ({reason})")]
    PathEscape {
        /// The offending entry path as read from the archive.
        path: std::path::PathBuf,
        /// What we caught: "absolute" or "parent-dir".
        reason: &'static str,
    },
    /// I/O failure. The `tar` crate (v0.4) surfaces all archive-level
    /// errors (corrupt header, truncated stream, malformed entry, etc.) as
    /// `std::io::Error`, so they land here too.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The artifact-dir hint in `[package.metadata.embassy].build.artifact-dir`
    /// pointed to a path that doesn't exist under the crate dir. This is
    /// always a manifest authoring error, distinct from "I scanned and
    /// found no ELF in the expected location" (`NoElf`).
    #[error("hint-dir does not exist: {dir}")]
    HintDirMissing {
        /// The fully-joined `crate_dir + artifact_dir` that we expected to find.
        dir: String,
    },
    /// Manifest parse error.
    #[error("manifest: {0}")]
    Manifest(String),
    /// `cargo build` failed; stderr captured.
    #[error("cargo build failed (exit {exit:?}); stderr:\n{stderr}")]
    Cargo {
        /// Exit code from `std::process::ExitStatus::code()`. `None` means the
        /// process was terminated by a signal (Unix) and has no exit code.
        exit: Option<i32>,
        /// Captured stderr (tail).
        stderr: String,
    },
    /// `cargo build` succeeded but no ELF could be located.
    #[error("no ELF artifact found in {dir}")]
    NoElf {
        /// Directory that was scanned.
        dir: String,
    },
}

/// Result alias.
pub type Result<T, E = BuildError> = std::result::Result<T, E>;
