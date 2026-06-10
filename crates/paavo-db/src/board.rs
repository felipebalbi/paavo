//! Board table typed helpers.

use crate::error::{DbError, Result};
use paavo_proto::{BoardHealth, BoardSelector, BoardSpec, ProbeSelector};
use rusqlite::{params, Connection, OptionalExtension, Row};

/// One row from the `board` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardRow {
    /// The publicly-shaped board spec.
    pub spec: BoardSpec,
    /// Free-form reason, set when `spec.health == Quarantined`.
    pub quarantine_reason: Option<String>,
    /// Counts toward auto-quarantine threshold (config:
    /// `quarantine.consecutive_infra_failures`).
    pub consecutive_infra_failures: u32,
    /// Last successful dispatch in epoch ms.
    pub last_used_at: Option<i64>,
    /// First-seen epoch ms.
    pub created_at: i64,
}

impl BoardRow {
    /// Insert a new board. Initial counters/values: 0 infra failures, no
    /// last_used_at, no quarantine reason.
    pub fn insert(conn: &Connection, spec: &BoardSpec, now_ms: i64) -> Result<()> {
        let probe_json = serde_json::to_string(&spec.probe_selector)?;
        conn.execute(
            "INSERT INTO board (
                id, kind, probe_selector, chip_name, target_name,
                wiring_profile, health, quarantine_reason,
                consecutive_infra_failures, last_used_at, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 0, NULL, ?8)",
            params![
                spec.id,
                spec.kind,
                probe_json,
                spec.chip_name,
                spec.target_name,
                spec.wiring_profile,
                health_to_str(spec.health),
                now_ms,
            ],
        )?;
        Ok(())
    }

    /// Fetch a single board by id. Errors if missing.
    pub fn get(conn: &Connection, id: &str) -> Result<Self> {
        // Outer `?` propagates `rusqlite::Error` (via `DbError::From`),
        // leaving the inner `Result<BoardRow>` produced by `from_row` —
        // which is exactly this function's return type.
        conn.query_row("SELECT * FROM board WHERE id = ?1", params![id], from_row)?
    }

    /// List all boards, ordered by id ascending.
    pub fn list_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare("SELECT * FROM board ORDER BY id ASC")?;
        let rows = stmt
            .query_map([], from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Find healthy boards matching the selector. Result is unordered; the
    /// scheduler decides LRU.
    pub fn find_healthy_for_selector(conn: &Connection, sel: &BoardSelector) -> Result<Vec<Self>> {
        let mut sql = String::from("SELECT * FROM board WHERE kind = ?1 AND health = 'healthy'");
        let mut next_param = 2;
        if sel.instance.is_some() {
            sql.push_str(&format!(" AND id = ?{next_param}"));
            next_param += 1;
        }
        if sel.wiring_profile.is_some() {
            sql.push_str(&format!(" AND wiring_profile = ?{next_param}"));
        }

        let mut stmt = conn.prepare(&sql)?;
        let mut bound: Vec<&dyn rusqlite::ToSql> = vec![&sel.kind];
        if let Some(inst) = &sel.instance {
            bound.push(inst);
        }
        if let Some(wp) = &sel.wiring_profile {
            bound.push(wp);
        }
        let rows = stmt
            .query_map(rusqlite::params_from_iter(bound), from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Find a board by id, returning `Ok(None)` if missing.
    pub fn find(conn: &Connection, id: &str) -> Result<Option<Self>> {
        let row = conn
            .query_row("SELECT * FROM board WHERE id = ?1", params![id], from_row)
            .optional()?;
        match row {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Update `last_used_at` to `now_ms`.
    pub fn touch_last_used(conn: &Connection, id: &str, now_ms: i64) -> Result<()> {
        conn.execute(
            "UPDATE board SET last_used_at = ?1 WHERE id = ?2",
            params![now_ms, id],
        )?;
        Ok(())
    }

    /// Increment `consecutive_infra_failures` by 1.
    pub fn bump_infra_failure(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE board SET consecutive_infra_failures =
             consecutive_infra_failures + 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Reset `consecutive_infra_failures` to 0. Called after a job whose
    /// outcome does not count toward infra failure (per
    /// `JobOutcome::counts_toward_infra_failure`).
    pub fn reset_infra_failures(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE board SET consecutive_infra_failures = 0 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Flip board to `quarantined` and record a reason.
    pub fn quarantine(conn: &Connection, id: &str, reason: &str) -> Result<()> {
        conn.execute(
            "UPDATE board SET health = 'quarantined', quarantine_reason = ?1
             WHERE id = ?2",
            params![reason, id],
        )?;
        Ok(())
    }

    /// Flip board back to `healthy`, clear quarantine reason and reset the
    /// infra failure counter.
    pub fn unquarantine(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE board SET health = 'healthy', quarantine_reason = NULL,
             consecutive_infra_failures = 0 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }
}

fn health_to_str(h: BoardHealth) -> &'static str {
    match h {
        BoardHealth::Healthy => "healthy",
        BoardHealth::Quarantined => "quarantined",
    }
}

fn health_from_str(s: &str) -> Result<BoardHealth> {
    match s {
        "healthy" => Ok(BoardHealth::Healthy),
        "quarantined" => Ok(BoardHealth::Quarantined),
        other => Err(DbError::UnknownEnum {
            column: "board.health",
            value: other.to_string(),
        }),
    }
}

/// Map a row to a Result, with JSON/enum decoding errors surfacing as
/// `DbError`.
fn from_row(r: &Row<'_>) -> rusqlite::Result<Result<BoardRow>> {
    let probe_json: String = r.get("probe_selector")?;
    let health_str: String = r.get("health")?;
    let id: String = r.get("id")?;
    let kind: String = r.get("kind")?;
    let chip_name: String = r.get("chip_name")?;
    let target_name: String = r.get("target_name")?;
    let wiring_profile: Option<String> = r.get("wiring_profile")?;
    let quarantine_reason: Option<String> = r.get("quarantine_reason")?;
    let raw_counter: i64 = r.get("consecutive_infra_failures")?;
    let last_used_at: Option<i64> = r.get("last_used_at")?;
    let created_at: i64 = r.get("created_at")?;

    Ok((|| -> Result<BoardRow> {
        let probe_selector: ProbeSelector = serde_json::from_str(&probe_json)?;
        let health = health_from_str(&health_str)?;
        let consecutive_infra_failures: u32 =
            raw_counter.try_into().map_err(|_| DbError::UnknownEnum {
                column: "board.consecutive_infra_failures",
                value: "negative or > u32::MAX".into(),
            })?;
        Ok(BoardRow {
            spec: BoardSpec {
                id,
                kind,
                probe_selector,
                chip_name,
                target_name,
                wiring_profile,
                health,
            },
            quarantine_reason,
            consecutive_infra_failures,
            last_used_at,
            created_at,
        })
    })())
}
