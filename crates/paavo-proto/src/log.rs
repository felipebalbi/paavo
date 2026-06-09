//! Log frame types as streamed from paavo-runner to paavo-core to paavo-cli.

use serde::{Deserialize, Serialize};

/// defmt log severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// `defmt::trace!`
    Trace,
    /// `defmt::debug!`
    Debug,
    /// `defmt::info!`
    Info,
    /// `defmt::warn!`
    Warn,
    /// `defmt::error!`
    Error,
}

/// One decoded defmt frame emitted by a running test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFrame {
    /// Monotonic sequence number per job, starting at 0.
    pub seq: u64,
    /// Microseconds since job start.
    pub ts_us: u64,
    /// Log severity.
    pub level: LogLevel,
    /// defmt `target` (Rust module path), if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Decoded message body.
    pub message: String,
}
