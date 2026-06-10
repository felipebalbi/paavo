//! ELF discovery from `[package.metadata.embassy]` or directory scan.

use crate::error::{BuildError, Result};
use std::path::{Path, PathBuf};

/// Optional manifest hint: `[package.metadata.embassy].build.artifact-dir`.
#[derive(Debug, Default, Clone)]
pub struct ManifestArtifactHint {
    /// Sub-path under the crate dir that is known to contain the ELF.
    pub artifact_dir: Option<PathBuf>,
}

/// Locate the ELF for a built crate. See Task 3.1.b for the implementation.
pub fn discover_elf(
    _crate_dir: &Path,
    _target_dir: &Path,
    _hint: &ManifestArtifactHint,
) -> Result<PathBuf> {
    Err(BuildError::NoElf {
        dir: "discover_elf is wired in Task 3.1.b".into(),
    })
}
