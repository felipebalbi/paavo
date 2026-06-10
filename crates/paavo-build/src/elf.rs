//! ELF discovery from `[package.metadata.embassy].build.artifact-dir` or
//! a fallback `target/release/` / `target/<triple>/release/` scan.

use crate::error::{BuildError, Result};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Optional manifest hint: `[package.metadata.embassy].build.artifact-dir`.
#[derive(Debug, Default, Clone)]
pub struct ManifestArtifactHint {
    /// Sub-path relative to the crate dir that is known to contain the ELF.
    pub artifact_dir: Option<PathBuf>,
}

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// Locate the ELF for a built crate.
///
/// Strategy:
/// 1. If `hint.artifact_dir` is set: scan `crate_dir/<artifact_dir>`
///    recursively for an ELF magic file. Prefer files ending in `.elf`.
/// 2. Otherwise scan `target_dir/release/` (host builds) or
///    `target_dir/<triple>/release/` (cross builds) for the same.
pub fn discover_elf(
    crate_dir: &Path,
    target_dir: &Path,
    hint: &ManifestArtifactHint,
) -> Result<PathBuf> {
    let scan_root = if let Some(artifact) = &hint.artifact_dir {
        let joined = crate_dir.join(artifact);
        if !joined.is_dir() {
            return Err(BuildError::HintDirMissing {
                dir: joined.display().to_string(),
            });
        }
        joined
    } else {
        match scan_release_dirs(target_dir) {
            Some(p) => p,
            None => {
                return Err(BuildError::NoElf {
                    dir: target_dir.display().to_string(),
                })
            }
        }
    };
    pick_elf(&scan_root)
}

/// Locate the release output directory under `target_dir`.
///
/// Two cases:
/// * **Host build** (no cross-compile): `cargo build --release` writes to
///   `target/release/` directly.
/// * **Cross build** (e.g. `--target thumbv8m.main-none-eabihf`): cargo
///   writes to `target/<triple>/release/`.
///
/// We check the bare `release/` first; if absent, we scan one level deep
/// for a sibling directory containing a `release/` subdir. Host wins if
/// both exist (which shouldn't happen in normal use).
fn scan_release_dirs(target_dir: &Path) -> Option<PathBuf> {
    let direct = target_dir.join("release");
    if direct.is_dir() {
        return Some(direct);
    }
    let mut entries: Vec<_> = std::fs::read_dir(target_dir).ok()?.flatten().collect();
    // Sort lexicographically by file name so the chosen triple-dir is
    // deterministic across machines/filesystems. read_dir order is OS-
    // and filesystem-dependent, which would otherwise make CI fragile.
    entries.sort_by_key(|e| e.file_name());
    for ent in entries {
        let release = ent.path().join("release");
        if release.is_dir() {
            return Some(release);
        }
    }
    None
}

/// Walk `root` (bounded depth) and return the best ELF candidate.
///
/// Bound: `min_depth=1, max_depth=3`. Artifacts can live in subdirs such as
/// `release/deps/` but never deeper. Among ELF-magic files, those with an
/// `.elf` extension are preferred (sorted last so `pop` picks them first).
fn pick_elf(root: &Path) -> Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(root)
        .min_depth(1)
        .max_depth(3)
        .into_iter()
        .flatten()
    {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if is_elf(p) {
            candidates.push(p.to_path_buf());
        }
    }
    // First pass: stable lexicographic order so within-extension-class
    // ordering is reproducible across machines/filesystems (WalkDir
    // traversal order is OS-dependent).
    candidates.sort();
    // Second pass: stable sort by `.elf`-extension; .elf files sort last
    // so `pop()` returns them first. `sort_by` is stable, so within each
    // class the lexicographic order from above is preserved.
    candidates.sort_by(|a, b| {
        let ax = a.extension().and_then(|s| s.to_str()) == Some("elf");
        let bx = b.extension().and_then(|s| s.to_str()) == Some("elf");
        ax.cmp(&bx)
    });
    candidates.pop().ok_or_else(|| BuildError::NoElf {
        dir: root.display().to_string(),
    })
}

/// Best-effort ELF magic check. Returns `false` on any I/O error (file
/// can't be opened, can't be read, too short) — discovery is a scan, not
/// a validator.
fn is_elf(p: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(p) else {
        return false;
    };
    use std::io::Read;
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() {
        return false;
    }
    magic == ELF_MAGIC
}
