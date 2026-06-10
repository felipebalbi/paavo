//! Errors returned by paavo-probe.

use thiserror::Error;

/// Errors during ELF parsing or probe operations.
#[derive(Debug, Error)]
pub enum ProbeError {
    /// `object` crate refused to parse the ELF.
    #[error("elf parse: {0}")]
    Elf(#[from] object::Error),
    /// A `.paavo.target` section was empty or contained no NUL-terminated
    /// string.
    #[error("`.paavo.target` section is empty or malformed")]
    EmptyTarget,
    /// A `.paavo.target` section had unexpected wire format (NUL-less,
    /// interior NUL with trailing bytes, or invalid UTF-8).
    #[error("`.paavo.target` section is malformed: {reason}")]
    MalformedTarget {
        /// Human-readable diagnostic (e.g. "missing trailing NUL",
        /// "interior NUL at byte 5 with 3 trailing bytes after",
        /// "invalid UTF-8 at byte 7").
        reason: String,
    },
    /// A `.paavo.timeout` / `.paavo.inactivity_timeout` section was
    /// not exactly 4 bytes (u32 LE).
    #[error("`{section}` section must be 4 bytes (u32 LE), got {got}")]
    BadIntegerSection {
        /// Section name.
        section: &'static str,
        /// Actual length.
        got: usize,
    },
    /// probe-rs connect or operation error (only used when the real adapter
    /// is in play; mocks never produce this).
    #[error("probe-rs: {0}")]
    ProbeRs(String),
    /// defmt-decoder failed to read or decode the symbol table.
    #[error("defmt decode: {0}")]
    Defmt(String),
}

/// Result alias used throughout the crate.
pub type Result<T, E = ProbeError> = std::result::Result<T, E>;
