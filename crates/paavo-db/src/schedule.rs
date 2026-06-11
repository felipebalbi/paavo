//! schedule table typed helpers.

use crate::error::Result;
use rusqlite::{params, Connection};

/// One row from the `schedule` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRow {
    /// Schedule id, e.g. `nightly`.
    pub id: String,
    /// Cron expression.
    pub cron: String,
    /// Whether the schedule is currently active.
    pub enabled: bool,
    /// Last firing time, epoch ms.
    pub last_triggered_at: Option<i64>,
    /// Last completion time, epoch ms.
    pub last_completed_at: Option<i64>,
}

/// Partial update used after a schedule firing or completion.
///
/// `None` means "leave the existing value alone". To explicitly clear a
/// timestamp, a dedicated operation would be needed (not in scope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleUpdate {
    /// New value for `last_triggered_at`, if any.
    pub last_triggered_at: Option<i64>,
    /// New value for `last_completed_at`, if any.
    pub last_completed_at: Option<i64>,
}

impl ScheduleRow {
    /// Insert or update a schedule row by id. On conflict, `cron` and
    /// `enabled` are refreshed; `last_triggered_at` and
    /// `last_completed_at` are coalesced — a non-NULL value from the
    /// upsert overwrites the existing one, a NULL leaves the existing
    /// value intact. This means the cron driver can push
    /// `last_triggered_at` on every fire via `upsert` without losing
    /// the value once `apply_update` writes `last_completed_at`.
    pub fn upsert(conn: &Connection, s: &ScheduleRow) -> Result<()> {
        conn.execute(
            "INSERT INTO schedule
                (id, cron, enabled, last_triggered_at, last_completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                cron = excluded.cron,
                enabled = excluded.enabled,
                last_triggered_at = COALESCE(excluded.last_triggered_at, last_triggered_at),
                last_completed_at = COALESCE(excluded.last_completed_at, last_completed_at)",
            params![
                s.id,
                s.cron,
                s.enabled as i64,
                s.last_triggered_at,
                s.last_completed_at,
            ],
        )?;
        Ok(())
    }

    /// Fetch one schedule by id. Returns `DbError::NotFound` on missing
    /// so the HTTP layer can map straight to 404 without pattern-
    /// matching on `rusqlite::Error::QueryReturnedNoRows`.
    pub fn get(conn: &Connection, id: &str) -> Result<ScheduleRow> {
        match conn.query_row(
            "SELECT id, cron, enabled, last_triggered_at, last_completed_at
             FROM schedule WHERE id = ?1",
            params![id],
            row_to_schedule,
        ) {
            Ok(row) => Ok(row),
            Err(rusqlite::Error::QueryReturnedNoRows) => Err(crate::error::DbError::NotFound {
                entity: "schedule",
                id: id.to_string(),
            }),
            Err(other) => Err(other.into()),
        }
    }

    /// List every schedule row, ordered by `id` ascending. Powers
    /// paavo-web's `/schedule` page, which renders the registered cron
    /// entries plus their last-trigger / last-complete timestamps.
    pub fn list_all(conn: &Connection) -> Result<Vec<ScheduleRow>> {
        let mut stmt = conn.prepare(
            "SELECT id, cron, enabled, last_triggered_at, last_completed_at
             FROM schedule ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map([], row_to_schedule)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Apply a partial update; fields set to `None` are not touched.
    pub fn apply_update(conn: &Connection, id: &str, u: &ScheduleUpdate) -> Result<()> {
        if let Some(t) = u.last_triggered_at {
            conn.execute(
                "UPDATE schedule SET last_triggered_at = ?1 WHERE id = ?2",
                params![t, id],
            )?;
        }
        if let Some(t) = u.last_completed_at {
            conn.execute(
                "UPDATE schedule SET last_completed_at = ?1 WHERE id = ?2",
                params![t, id],
            )?;
        }
        Ok(())
    }
}

/// Decode one row from a `SELECT id, cron, enabled, last_triggered_at,
/// last_completed_at FROM schedule` query. Shared by `ScheduleRow::get`
/// and `ScheduleRow::list_all` so a future column addition is a
/// single-site edit (same pattern as `BoardRow::from_row`).
fn row_to_schedule(r: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduleRow> {
    Ok(ScheduleRow {
        id: r.get("id")?,
        cron: r.get("cron")?,
        enabled: r.get::<_, i64>("enabled")? == 1,
        last_triggered_at: r.get("last_triggered_at")?,
        last_completed_at: r.get("last_completed_at")?,
    })
}
