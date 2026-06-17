//! `Db` — the owned SQLite handle. Single writer (paavod), single reader
//! (paavo-web). WAL mode + busy timeout.

use crate::error::{DbError, Result};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, Error, OpenFlags};
use std::path::Path;

mod embedded {
    refinery::embed_migrations!("./migrations");
}

/// Owned SQLite handle, plus migration bookkeeping.
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open (or create) a read-write SQLite database at `path` and run any
    /// pending migrations. WAL journal mode, 5 s busy timeout, foreign keys on.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut conn = Connection::open(path.as_ref())?;
        configure(&mut conn, /* readonly = */ false)?;
        embedded::migrations::runner()
            .run(&mut conn)
            .map_err(DbError::from)?;
        Ok(Self { conn })
    }

    /// Open `path` read-only. Caller (paavo-web) must wait for paavod to have
    /// created the file first.
    pub fn open_readonly<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut conn = Connection::open_with_flags(
            path.as_ref(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?;
        configure(&mut conn, /* readonly = */ true)?;
        Ok(Self { conn })
    }

    /// Raw connection accessor — bypasses the typed query helpers in
    /// `board.rs`, `job.rs`, etc. Use only for tests or for queries the
    /// typed surface does not yet cover.
    pub fn raw_conn(&self) -> &Connection {
        &self.conn
    }

    /// Raw connection accessor — bypasses the typed query helpers in
    /// `board.rs`, `job.rs`, etc. Use only for tests or for queries the
    /// typed surface does not yet cover.
    pub fn raw_conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

fn configure(conn: &mut Connection, readonly: bool) -> Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    if !readonly {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
    }
    register_fuzzy_score(conn)?;
    Ok(())
}

/// Register `fuzzy_score(haystack, needle) -> Option<i64>`: the
/// `SkimMatcherV2` ranking the web UI used to run in memory, now callable
/// from SQL by `JobRow::search_index_page`. Registered on every connection
/// (RW + RO); only the RO search path invokes it. `None` (no match) maps to
/// SQL `NULL`.
fn register_fuzzy_score(conn: &Connection) -> Result<()> {
    // Built once and captured: the function runs per matched row, so
    // rebuilding the matcher each call would waste its scoring tables.
    // SkimMatcherV2 is plain config data (Send + 'static); the closure bounds
    // are satisfied and create_scalar_function is safe (no `unsafe`).
    let matcher = SkimMatcherV2::default();
    conn.create_scalar_function(
        "fuzzy_score",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx| {
            // get_raw().as_str() borrows the column bytes — no per-row String.
            let haystack = ctx
                .get_raw(0)
                .as_str()
                .map_err(|e| Error::UserFunctionError(e.into()))?;
            let needle = ctx
                .get_raw(1)
                .as_str()
                .map_err(|e| Error::UserFunctionError(e.into()))?;
            Ok(matcher.fuzzy_match(haystack, needle))
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn fuzzy_score_some_for_subsequence_none_otherwise() {
        let dir = tempdir().unwrap();
        let db = Db::open(dir.path().join("t.sqlite")).unwrap();
        let c = db.raw_conn();
        let hit: Option<i64> = c
            .query_row("SELECT fuzzy_score('alice mcxa266-01', 'almcx')", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            hit.is_some(),
            "almcx is a subsequence of 'alice mcxa266-01'"
        );
        let miss: Option<i64> = c
            .query_row("SELECT fuzzy_score('bob', 'almcx')", [], |r| r.get(0))
            .unwrap();
        assert!(miss.is_none(), "almcx is not a subsequence of 'bob'");
    }
}
