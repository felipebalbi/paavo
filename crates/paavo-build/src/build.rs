//! `cargo build --release` invocation.

use crate::error::Result;
use std::path::PathBuf;

/// Build plan derived from a `JobSpec` and a sandbox directory.
#[derive(Debug, Clone)]
pub struct BuildPlan {
    /// Sandbox dir containing the unpacked crate.
    pub crate_dir: PathBuf,
    /// `CARGO_TARGET_DIR` to share across jobs for incremental reuse.
    pub target_dir: PathBuf,
    /// Optional `cargo update -p ...` packages to refresh before building
    /// (used by soak-test corpora that track `embassy-rs/embassy` main).
    pub cargo_update_packages: Vec<String>,
}

/// What `build_release` returns.
#[derive(Debug, Clone)]
pub struct BuildResult {
    /// Path to the discovered ELF.
    pub elf_path: PathBuf,
    /// Size of the ELF on disk, bytes.
    pub elf_size_bytes: u64,
    /// Captured stdout/stderr (useful for surfacing build warnings).
    pub stderr_tail: String,
}

/// Invoke `cargo build --release` in `plan.crate_dir`, then discover the ELF.
///
/// Implemented fully in Task 3.1.c so the test in 3.1.a doesn't depend on
/// `cargo` being on PATH.
pub fn build_release(_plan: &BuildPlan) -> Result<BuildResult> {
    Err(crate::error::BuildError::Cargo {
        exit: None,
        stderr: "build_release is wired in Task 3.1.c".into(),
    })
}
