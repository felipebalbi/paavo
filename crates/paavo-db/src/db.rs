//! `Db` — the owned SQLite handle. Single writer (paavod), single reader
//! (paavo-web). WAL mode + busy timeout.

use crate::error::{DbError, Result};
use rusqlite::{Connection, OpenFlags};
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
    Ok(())
}
