//! Build-cache helpers: pair `paavo-build` (which produces ELFs) with
//! `paavo-db::BuildCacheEntry` (which persists where the ELF landed).
//!
//! Lives in `paavo-core` because it bridges the two; `paavo-build` itself
//! stays DB-free per spec §4.1.

use crate::error::Result;
use paavo_db::BuildCacheEntry;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Outcome of a cache lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheLookup {
    /// Cache hit. Caller can skip `paavo_build::build_release`.
    Hit {
        /// Cached ELF.
        elf_path: PathBuf,
    },
    /// Cache miss.
    Miss,
}

/// Look up a tar's cached ELF by blake3. Returns `Miss` if there's no row,
/// or if the row's ELF file has gone missing on disk (in which case the
/// stale row is also pruned so this function is self-healing).
///
/// `now_ms` is the wall-clock instant to record as `last_used_at` on a
/// hit. Production passes `Utc::now().timestamp_millis()`; tests inject
/// deterministic values.
///
/// **Single-writer assumption.** The `find → is_file → touch_last_used`
/// triple is three operations; concurrent writers could observe an
/// inconsistent snapshot (`evict_lru` between `is_file` and
/// `touch_last_used` would let us return `Hit { elf_path }` over an ELF
/// that has just been unlinked, with the subsequent `touch_last_used`
/// silently no-op'ing on a missing row). paavod's dispatch thread is
/// single-threaded, so in production there is exactly one writer here.
/// Callers that introduce concurrent access must serialize against
/// [`evict_lru`].
pub fn cache_lookup(conn: &Connection, tar_blake3: &str, now_ms: i64) -> Result<CacheLookup> {
    let Some(entry) = BuildCacheEntry::find(conn, tar_blake3)? else {
        return Ok(CacheLookup::Miss);
    };
    let elf_path = PathBuf::from(&entry.elf_path);
    if !elf_path.is_file() {
        // Self-heal: drop the stale row. Errors here are best-effort —
        // the user-visible answer is still Miss.
        let _ = BuildCacheEntry::delete(conn, tar_blake3);
        return Ok(CacheLookup::Miss);
    }
    BuildCacheEntry::touch_last_used(conn, tar_blake3, now_ms)?;
    Ok(CacheLookup::Hit { elf_path })
}

/// Insert (or refresh) a cache entry mapping `tar_blake3 -> elf_path`.
/// Stats the ELF file to record its size; failure to stat returns
/// `CoreError::Io`. Rejects non-regular files (directories, etc.) with
/// `CoreError::Io(ErrorKind::InvalidInput)`.
pub fn cache_store(
    conn: &Connection,
    tar_blake3: &str,
    elf_path: &Path,
    now_ms: i64,
) -> Result<()> {
    let meta = std::fs::metadata(elf_path)?;
    if !meta.is_file() {
        return Err(crate::CoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("cache_store: not a regular file: {}", elf_path.display()),
        )));
    }
    let size = meta.len();
    BuildCacheEntry::upsert(
        conn,
        &BuildCacheEntry {
            tar_blake3: tar_blake3.to_string(),
            elf_path: elf_path.display().to_string(),
            built_at: now_ms,
            last_used_at: now_ms,
            size_bytes: size,
        },
    )?;
    Ok(())
}

/// Evict cache entries until total size <= `max_bytes`. Removes the
/// underlying ELF files on disk for each evicted row. Returns the list
/// of evicted entries (in eviction order — least-recently-used first).
///
/// Best-effort on file deletion: if an ELF file is already gone, the
/// eviction still counts as successful (the DB row is removed regardless).
pub fn evict_lru(conn: &Connection, max_bytes: u64) -> Result<Vec<BuildCacheEntry>> {
    let evicted = BuildCacheEntry::evict_until_under(conn, max_bytes)?;
    for entry in &evicted {
        let _ = std::fs::remove_file(&entry.elf_path);
    }
    Ok(evicted)
}
