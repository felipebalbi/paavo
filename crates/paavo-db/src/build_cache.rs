//! build_cache table typed helpers and LRU eviction policy.

use crate::error::{DbError, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};

/// One row from the `build_cache` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCacheEntry {
    /// blake3 of the input tar.
    pub tar_blake3: String,
    /// On-disk ELF location.
    pub elf_path: String,
    /// First-built time, epoch ms.
    pub built_at: i64,
    /// Last-accessed time, epoch ms (drives LRU).
    pub last_used_at: i64,
    /// Disk footprint of the cached ELF, in bytes.
    pub size_bytes: u64,
}

/// Aggregate stats for the build-cache LRU policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildCacheStats {
    /// Sum of recorded `size_bytes` across all entries. Reflects the size at
    /// upsert time, not a live disk measurement — drifts if cached ELFs are
    /// mutated externally.
    pub total_bytes: u64,
    /// Number of entries.
    pub count: u64,
}

impl BuildCacheEntry {
    /// Insert or update an entry keyed by `tar_blake3`. On conflict
    /// `elf_path`, `last_used_at`, and `size_bytes` are refreshed; `built_at`
    /// is preserved as the first-built timestamp.
    pub fn upsert(conn: &Connection, e: &BuildCacheEntry) -> Result<()> {
        conn.execute(
            "INSERT INTO build_cache
                (tar_blake3, elf_path, built_at, last_used_at, size_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(tar_blake3) DO UPDATE SET
                elf_path = excluded.elf_path,
                last_used_at = excluded.last_used_at,
                size_bytes = excluded.size_bytes",
            params![
                e.tar_blake3,
                e.elf_path,
                e.built_at,
                e.last_used_at,
                e.size_bytes as i64,
            ],
        )?;
        Ok(())
    }

    /// Fetch an entry; errors if missing.
    pub fn get(conn: &Connection, tar_blake3: &str) -> Result<BuildCacheEntry> {
        let e = conn.query_row(
            "SELECT tar_blake3, elf_path, built_at, last_used_at, size_bytes
             FROM build_cache WHERE tar_blake3 = ?1",
            params![tar_blake3],
            row_to_entry,
        )?;
        Ok(e)
    }

    /// Find an entry, returning `Ok(None)` if missing.
    pub fn find(conn: &Connection, tar_blake3: &str) -> Result<Option<BuildCacheEntry>> {
        let row = conn
            .query_row(
                "SELECT tar_blake3, elf_path, built_at, last_used_at, size_bytes
                 FROM build_cache WHERE tar_blake3 = ?1",
                params![tar_blake3],
                row_to_entry,
            )
            .optional()?;
        Ok(row)
    }

    /// Update `last_used_at` for an existing entry. No-op if the row is gone.
    pub fn touch_last_used(conn: &Connection, tar_blake3: &str, now_ms: i64) -> Result<()> {
        conn.execute(
            "UPDATE build_cache SET last_used_at = ?1 WHERE tar_blake3 = ?2",
            params![now_ms, tar_blake3],
        )?;
        Ok(())
    }

    /// Aggregate stats across all entries.
    pub fn stats(conn: &Connection) -> Result<BuildCacheStats> {
        let (total_raw, count_raw): (i64, i64) = conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0), COUNT(*) FROM build_cache",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let total_bytes: u64 = total_raw.try_into().map_err(|_| DbError::UnknownEnum {
            column: "build_cache.size_bytes(sum)",
            value: "negative".into(),
        })?;
        let count: u64 = count_raw.try_into().map_err(|_| DbError::UnknownEnum {
            column: "build_cache.count",
            value: "negative".into(),
        })?;
        Ok(BuildCacheStats { total_bytes, count })
    }

    /// Delete a single cache entry by tar_blake3. No-op if no row matches.
    /// Returns `Ok(true)` if a row was deleted, `Ok(false)` if not.
    pub fn delete(conn: &Connection, tar_blake3: &str) -> Result<bool> {
        let n = conn.execute(
            "DELETE FROM build_cache WHERE tar_blake3 = ?1",
            rusqlite::params![tar_blake3],
        )?;
        Ok(n > 0)
    }

    /// Drop the least-recently-used entries until total bytes ≤ `max_bytes`,
    /// returning the evicted entries in eviction order (oldest first).
    ///
    /// ## Caller contract
    ///
    /// The caller MUST `std::fs::remove_file(&entry.elf_path)` for every
    /// returned entry to reclaim disk. Without that step the DB row is gone
    /// but the ELF file leaks. Treat `remove_file` failures as warnings
    /// (log and continue) — a later reconciliation pass (TODO below) handles
    /// the general orphan case anyway.
    ///
    /// ## Atomicity
    ///
    /// Each delete is a separate transaction (rusqlite auto-commits per
    /// statement under WAL). If the caller crashes mid-loop OR the function
    /// returns `Err` after partial progress, the DB rows are gone but the
    /// ELFs are still on disk. The cache self-heals (next build repopulates
    /// the row), so the only consequence is unbounded disk growth across
    /// crashes.
    ///
    /// TODO(later): add a disk-scan reconciliation pass — walk
    /// `cache_elfs_dir` and delete any ELF whose `tar_blake3` does not match
    /// a row in `build_cache`. That handles this case plus the symmetric
    /// case where `cache_store` succeeds in DB but the matching `fs::write`
    /// later got truncated.
    pub fn evict_until_under(conn: &Connection, max_bytes: u64) -> Result<Vec<BuildCacheEntry>> {
        let mut evicted = Vec::new();
        loop {
            let st = Self::stats(conn)?;
            if st.total_bytes <= max_bytes {
                return Ok(evicted);
            }
            let victim = conn
                .query_row(
                    "SELECT tar_blake3, elf_path, built_at, last_used_at, size_bytes
                     FROM build_cache ORDER BY last_used_at ASC LIMIT 1",
                    [],
                    row_to_entry,
                )
                .optional()?;
            let Some(victim) = victim else {
                // Stats said we're over cap, but the table is empty. Nothing
                // we can do; bail out cleanly rather than spinning forever.
                return Ok(evicted);
            };
            conn.execute(
                "DELETE FROM build_cache WHERE tar_blake3 = ?1",
                params![victim.tar_blake3],
            )?;
            evicted.push(victim);
        }
    }
}

fn row_to_entry(r: &Row<'_>) -> rusqlite::Result<BuildCacheEntry> {
    let size_raw: i64 = r.get(4)?;
    let size_bytes: u64 = size_raw
        .try_into()
        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(4, size_raw))?;
    Ok(BuildCacheEntry {
        tar_blake3: r.get(0)?,
        elf_path: r.get(1)?,
        built_at: r.get(2)?,
        last_used_at: r.get(3)?,
        size_bytes,
    })
}
