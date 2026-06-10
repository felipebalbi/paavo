//! `cargo build --release` invocation, with stderr capture and ELF discovery
//! handoff.

use crate::elf::{discover_elf, ManifestArtifactHint};
use crate::error::{BuildError, Result};
use std::path::PathBuf;
use std::process::Command;

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
    /// Captured stderr tail (last 8 KiB).
    pub stderr_tail: String,
}

/// Invoke `cargo build --release` in `plan.crate_dir`, then discover the ELF.
///
/// Steps:
/// 1. For each package in `plan.cargo_update_packages`, run
///    `cargo update -p <pkg>` (used by soak corpora that track a moving
///    upstream like `embassy-rs/embassy` main).
/// 2. Run `cargo build --release` with `CARGO_TARGET_DIR=plan.target_dir`.
/// 3. On failure, capture the last 8 KiB of stderr into
///    [`BuildError::Cargo`].
/// 4. On success, locate the ELF via
///    [`discover_elf`](crate::discover_elf) using a default (no hint)
///    [`ManifestArtifactHint`], which falls back to scanning the release
///    output dir under `target_dir`.
///
/// The `cargo` binary is selected from the `CARGO` env var (set by cargo
/// itself when running tests/build scripts) and falls back to plain
/// `"cargo"` on `$PATH`. This is the "cargo spawns cargo" idiom that lets
/// the workspace's pinned toolchain version flow through.
pub fn build_release(plan: &BuildPlan) -> Result<BuildResult> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());

    for pkg in &plan.cargo_update_packages {
        // Mirror the build path: capture stderr so soak operators can see
        // the real cargo diagnostic on failure, not a hard-coded sentinel.
        run_cargo(&cargo, &["update", "-p", pkg], plan)?;
    }

    let stderr_tail = run_cargo(&cargo, &["build", "--release"], plan)?;

    let hint = ManifestArtifactHint::default();
    let elf_path = discover_elf(&plan.crate_dir, &plan.target_dir, &hint)?;
    let elf_size_bytes = std::fs::metadata(&elf_path)?.len();
    Ok(BuildResult {
        elf_path,
        elf_size_bytes,
        stderr_tail,
    })
}

/// Run a cargo subcommand with stderr capture. Returns the (success-path)
/// stderr tail (≤ 8 KiB) or a `BuildError::Cargo` carrying the failure exit
/// code and stderr tail.
fn run_cargo(cargo: &std::ffi::OsStr, args: &[&str], plan: &BuildPlan) -> Result<String> {
    let output = Command::new(cargo)
        .args(args)
        .current_dir(&plan.crate_dir)
        .env("CARGO_TARGET_DIR", &plan.target_dir)
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr_tail = tail(&stderr, 8 * 1024);
    if !output.status.success() {
        return Err(BuildError::Cargo {
            exit: output.status.code(),
            stderr: stderr_tail,
        });
    }
    Ok(stderr_tail)
}

/// Truncate `s` to at most `max_bytes` from the end, respecting UTF-8
/// character boundaries (so we never split a multibyte codepoint and end
/// up with invalid UTF-8 in a captured stderr tail).
fn tail(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let start = s.len() - max_bytes;
    let mut idx = start;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    s[idx..].to_string()
}
