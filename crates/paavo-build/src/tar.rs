//! Tar unpacking with path-escape rejection, plus blake3 hashing of the
//! raw tar bytes (used as the build-cache key).

use crate::error::{BuildError, Result};
use std::path::{Component, Path, PathBuf};

/// Stable hex digest of `bytes` (typically the raw tar archive). Caller
/// uses this as the `paavo_db::build_cache` key.
pub fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Unpack `bytes` into `dst`, returning the directory we unpacked into.
///
/// Rejects entries whose path escapes `dst` via `..` or absolute paths.
///
/// **Non-atomic.** On error, `dst` may have been created and may contain
/// partially-extracted entries from before the rejection. Callers that
/// need cleanup should pass a `tempfile::TempDir` (which deletes on drop)
/// or remove `dst` after a failed call.
pub fn unpack_into(bytes: &[u8], dst: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dst)?;
    let mut archive = tar::Archive::new(bytes);
    for entry in archive.entries()? {
        let mut e = entry?;
        let path = e.path()?.into_owned();
        validate_path(&path)?;
        e.unpack_in(dst)?;
    }
    Ok(dst.to_path_buf())
}

fn validate_path(p: &Path) -> Result<()> {
    if p.is_absolute() {
        return Err(BuildError::PathEscape {
            path: p.to_path_buf(),
            reason: "absolute",
        });
    }
    for comp in p.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(BuildError::PathEscape {
                path: p.to_path_buf(),
                reason: "parent-dir",
            });
        }
    }
    Ok(())
}
