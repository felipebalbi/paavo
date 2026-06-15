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
/// 2. Otherwise enumerate release output directories under `target_dir`
///    in priority order (triple subdirs first, bare `release/` last)
///    and try each one — the first that yields an ELF wins.
pub fn discover_elf(
    crate_dir: &Path,
    target_dir: &Path,
    hint: &ManifestArtifactHint,
) -> Result<PathBuf> {
    if let Some(artifact) = &hint.artifact_dir {
        let joined = crate_dir.join(artifact);
        if !joined.is_dir() {
            return Err(BuildError::HintDirMissing {
                dir: joined.display().to_string(),
            });
        }
        return pick_elf(&joined);
    }

    let candidates = release_dirs_in_priority_order(target_dir);
    if candidates.is_empty() {
        return Err(BuildError::NoElf {
            dir: target_dir.display().to_string(),
        });
    }
    let mut total_scanned = 0usize;
    for root in &candidates {
        match try_pick_elf(root) {
            PickResult::Found(p) => return Ok(p),
            PickResult::Empty { scanned } => total_scanned += scanned,
        }
    }
    // No ELF in any candidate root. Surface what we looked at so the
    // operator can tell "cross-compile produced nothing" from
    // "discovery scanned the wrong dir".
    let roots = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(BuildError::NoElf {
        dir: format!(
            "scanned {} file(s) under [{}] but found no ELF magic; \
             check that the crate cross-compiles to an embedded target \
             (`cargo build --release` from the crate dir should produce \
             an ELF under `target/<triple>/release/`)",
            total_scanned, roots
        ),
    })
}

/// Return every plausible `release/` directory under `target_dir`, in
/// the order they should be searched.
///
/// Cargo's layout:
/// * Pure-host build: only `target/release/` exists.
/// * Pure cross-compile (no proc-macro deps): only
///   `target/<triple>/release/` exists.
/// * Cross-compile WITH proc-macro deps: both exist — `target/release/`
///   holds host-built proc-macro `.rlib`/`.dll` files (no ELF), and
///   `target/<triple>/release/` holds the actual artifact.
///
/// Therefore: triple subdirs first (sorted lexicographically for
/// determinism), bare `release/` last as the fallback.
fn release_dirs_in_priority_order(target_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // Triple-subdir candidates: any direct child of `target_dir` that
    // itself contains a `release/` subdir. Sorted by file name so the
    // chosen triple-dir is deterministic across machines/filesystems
    // (read_dir order is OS-dependent).
    if let Ok(entries) = std::fs::read_dir(target_dir) {
        let mut subdirs: Vec<_> = entries.flatten().collect();
        subdirs.sort_by_key(|e| e.file_name());
        for ent in subdirs {
            let p = ent.path();
            if p.file_name().and_then(|s| s.to_str()) == Some("release") {
                continue; // handled below as the bare-release fallback
            }
            let release = p.join("release");
            if release.is_dir() {
                out.push(release);
            }
        }
    }
    // Bare `target/release/` as the last-resort fallback. Only used for
    // pure-host crates that have no `[build] target = "..."` config and
    // no `--target` flag — vanishingly rare for paavo's intended
    // workload (DUT firmware), but supported for tests.
    let bare = target_dir.join("release");
    if bare.is_dir() {
        out.push(bare);
    }
    out
}

/// Result of a single root's ELF scan.
enum PickResult {
    /// At least one ELF was found.
    Found(PathBuf),
    /// No ELFs in this root; `scanned` is the count of files
    /// inspected (for diagnostic accounting in the all-roots-empty
    /// fallback path).
    Empty { scanned: usize },
}

/// Walk `root` (bounded depth) and return the best ELF candidate, or
/// `Empty { scanned }` if no file under `root` carries ELF magic.
fn try_pick_elf(root: &Path) -> PickResult {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut scanned = 0usize;
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
        scanned += 1;
        if is_elf(p) {
            candidates.push(p.to_path_buf());
        }
    }
    candidates.sort();
    candidates.sort_by(|a, b| {
        let ax = a.extension().and_then(|s| s.to_str()) == Some("elf");
        let bx = b.extension().and_then(|s| s.to_str()) == Some("elf");
        ax.cmp(&bx)
    });
    match candidates.pop() {
        Some(p) => PickResult::Found(p),
        None => PickResult::Empty { scanned },
    }
}

/// Walk `root` (bounded depth) and return the best ELF candidate.
///
/// Bound: `min_depth=1, max_depth=3`. Artifacts can live in subdirs such as
/// `release/deps/` but never deeper. Among ELF-magic files, those with an
/// `.elf` extension are preferred (sorted last so `pop` picks them first).
///
/// Used by the hint-directory path; the no-hint path uses
/// [`try_pick_elf`] so it can try multiple roots in priority order.
fn pick_elf(root: &Path) -> Result<PathBuf> {
    match try_pick_elf(root) {
        PickResult::Found(p) => Ok(p),
        PickResult::Empty { .. } => Err(BuildError::NoElf {
            dir: root.display().to_string(),
        }),
    }
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
