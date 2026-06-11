//! Layout under `server.state_dir`.

use std::path::{Path, PathBuf};

/// Resolved sub-paths inside the daemon state directory.
#[derive(Debug, Clone)]
pub struct StateDir {
    /// Root.
    pub root: PathBuf,
    /// SQLite database file.
    pub sqlite_path: PathBuf,
    /// Tar uploads keyed by blake3.
    pub uploads_dir: PathBuf,
    /// Per-job sandbox dirs.
    pub sandboxes_dir: PathBuf,
    /// Shared `CARGO_TARGET_DIR`.
    pub cargo_target_dir: PathBuf,
    /// Cached ELFs keyed by blake3.
    pub cache_elfs_dir: PathBuf,
    /// boards.toml — managed by `paavo-cli board add`.
    pub boards_toml: PathBuf,
}

impl StateDir {
    /// Compute paths under `root`; does not create them.
    pub fn from_root(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            root: root.to_path_buf(),
            sqlite_path: root.join("paavo.sqlite"),
            uploads_dir: root.join("uploads"),
            sandboxes_dir: root.join("sandboxes"),
            cargo_target_dir: root.join("cargo-target"),
            cache_elfs_dir: root.join("cache").join("elf"),
            boards_toml: root.join("boards.toml"),
        }
    }

    /// Create `root` and every subdirectory under it. Idempotent. Does
    /// NOT touch `sqlite_path` (created by paavo-db) or `boards_toml`
    /// (created by paavo-cli). TODO(M4.4): also chmod the root to 0700
    /// on Unix once paavod's main wires this up at startup.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        std::fs::create_dir_all(&self.uploads_dir)?;
        std::fs::create_dir_all(&self.sandboxes_dir)?;
        std::fs::create_dir_all(&self.cargo_target_dir)?;
        std::fs::create_dir_all(&self.cache_elfs_dir)?;
        Ok(())
    }
}
