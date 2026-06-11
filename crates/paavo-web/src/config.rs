//! paavo-web only reads the bits of paavo.toml it needs (state_dir + bind).

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Subset of paavo.toml relevant to the UI.
#[derive(Debug, Clone, Deserialize)]
pub struct RootConfig {
    /// `[server]` (state_dir).
    pub server: ServerSection,
    /// `[web]` (bind).
    pub web: WebSection,
}

/// `[server]`.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerSection {
    /// State dir containing paavo.sqlite.
    pub state_dir: PathBuf,
}

/// `[web]`.
#[derive(Debug, Clone, Deserialize)]
pub struct WebSection {
    /// `host:port`.
    pub bind: String,
}

impl RootConfig {
    /// Load from path.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let raw = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        toml::from_str(&raw).context("parsing paavo.toml")
    }
}
