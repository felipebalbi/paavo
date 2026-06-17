//! Board table typed helpers.

use crate::error::{DbError, Result};
use paavo_proto::{BoardHealth, BoardSelector, BoardSpec, ProbeSelector};
use rusqlite::{params, Connection, Error as RusqliteError, ErrorCode, OptionalExtension, Row};

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
    /// last_used_at, no quarantine reason. Returns
    /// `DbError::AlreadyExists` if the primary key already exists.
    pub fn insert(conn: &Connection, spec: &BoardSpec, now_ms: i64) -> Result<()> {
        let probe_json = serde_json::to_string(&spec.probe_selector)?;
        let result = conn.execute(
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
        );
        match result {
            Ok(_) => Ok(()),
            Err(RusqliteError::SqliteFailure(e, _)) if e.code == ErrorCode::ConstraintViolation => {
                Err(DbError::AlreadyExists {
                    entity: "board",
                    id: spec.id.clone(),
                })
            }
            Err(other) => Err(other.into()),
        }
    }

    /// Fetch a single board by id. Returns `DbError::NotFound` if
    /// missing so the HTTP layer can map straight to 404 without
    /// pattern-matching on `rusqlite::Error::QueryReturnedNoRows`.
    pub fn get(conn: &Connection, id: &str) -> Result<Self> {
        match conn.query_row("SELECT * FROM board WHERE id = ?1", params![id], from_row) {
            Ok(row_result) => row_result,
            Err(rusqlite::Error::QueryReturnedNoRows) => Err(DbError::NotFound {
                entity: "board",
                id: id.to_string(),
            }),
            Err(other) => Err(other.into()),
        }
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

    /// Page of boards ordered by id ascending. When `filter` is `Some(q)`
    /// (and non-empty after trimming), only boards whose `id` OR `kind`
    /// contains `q` (case-insensitive ASCII substring) are returned. The
    /// board fleet is small and id-stable (no live churn like the jobs
    /// table), so a plain `LIMIT/OFFSET` on the same `id ASC` order as
    /// `list_all` is sufficient — no `as_of` pin is needed.
    pub fn list_page(
        conn: &Connection,
        filter: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<Self>> {
        let needle = filter.map(|s| s.trim()).filter(|s| !s.is_empty());
        let (sql, bind): (String, Vec<rusqlite::types::Value>) = match needle {
            Some(q) => {
                let like = format!("%{}%", escape_like(q));
                (
                    "SELECT * FROM board WHERE (id LIKE ?1 ESCAPE '\\' OR kind LIKE ?1 ESCAPE '\\') \
                     ORDER BY id ASC LIMIT ?2 OFFSET ?3"
                        .into(),
                    vec![like.into(), (limit as i64).into(), (offset as i64).into()],
                )
            }
            None => (
                "SELECT * FROM board ORDER BY id ASC LIMIT ?1 OFFSET ?2".into(),
                vec![(limit as i64).into(), (offset as i64).into()],
            ),
        };
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(bind), from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Total board count, optionally filtered exactly like [`list_page`]
    /// (`id`/`kind` case-insensitive substring). Paired with `list_page`
    /// so paavo-web can render the total page count for the (filtered)
    /// boards list.
    pub fn count(conn: &Connection, filter: Option<&str>) -> Result<u64> {
        let needle = filter.map(|s| s.trim()).filter(|s| !s.is_empty());
        let n: i64 = match needle {
            Some(q) => {
                let like = format!("%{}%", escape_like(q));
                conn.query_row(
                    "SELECT COUNT(*) FROM board WHERE (id LIKE ?1 ESCAPE '\\' OR kind LIKE ?1 ESCAPE '\\')",
                    params![like],
                    |r| r.get(0),
                )?
            }
            None => conn.query_row("SELECT COUNT(*) FROM board", [], |r| r.get(0))?,
        };
        Ok(n as u64)
    }

    /// Total board count and how many are quarantined, in one pass.
    /// Healthy is derived on the wire type (`total - quarantined`).
    pub fn health_counts(conn: &Connection) -> Result<paavo_proto::BoardHealthCounts> {
        let (total, quarantined): (i64, i64) = conn.query_row(
            "SELECT COUNT(*), \
             COALESCE(SUM(CASE WHEN health = 'quarantined' THEN 1 ELSE 0 END), 0) \
             FROM board",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(paavo_proto::BoardHealthCounts {
            total: total as u64,
            quarantined: quarantined as u64,
        })
    }

    /// Find healthy boards matching the selector AND not currently
    /// dispatched (no `job` row in `running` state on this board — only
    /// the run phase holds a board; the build phase is board-free).
    /// Result is unordered; the scheduler decides LRU.
    ///
    /// The board-exclusivity clause (`NOT EXISTS (...)`) is what
    /// stops the dispatcher from launching two jobs on the same probe
    /// concurrently. Without it `pick_next` could happily return the
    /// same board twice in quick succession because `health` stays
    /// `Healthy` while a job runs.
    pub fn find_healthy_for_selector(conn: &Connection, sel: &BoardSelector) -> Result<Vec<Self>> {
        let mut sql = String::from(
            "SELECT * FROM board WHERE kind = ?1 AND health = 'healthy'
             AND NOT EXISTS (
                SELECT 1 FROM job WHERE job.board_id = board.id
                  AND job.state = 'running'
             )",
        );
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

    /// Update `last_used_at` to `now_ms`. Returns `DbError::NotFound`
    /// if `id` does not exist.
    pub fn touch_last_used(conn: &Connection, id: &str, now_ms: i64) -> Result<()> {
        let n = conn.execute(
            "UPDATE board SET last_used_at = ?1 WHERE id = ?2",
            params![now_ms, id],
        )?;
        require_one_row(n, id)
    }

    /// Increment `consecutive_infra_failures` by 1. Returns
    /// `DbError::NotFound` if `id` does not exist.
    pub fn bump_infra_failure(conn: &Connection, id: &str) -> Result<()> {
        let n = conn.execute(
            "UPDATE board SET consecutive_infra_failures =
             consecutive_infra_failures + 1 WHERE id = ?1",
            params![id],
        )?;
        require_one_row(n, id)
    }

    /// Reset `consecutive_infra_failures` to 0. Called after a job whose
    /// outcome does not count toward infra failure (per
    /// `JobOutcome::counts_toward_infra_failure`). Returns
    /// `DbError::NotFound` if `id` does not exist.
    pub fn reset_infra_failures(conn: &Connection, id: &str) -> Result<()> {
        let n = conn.execute(
            "UPDATE board SET consecutive_infra_failures = 0 WHERE id = ?1",
            params![id],
        )?;
        require_one_row(n, id)
    }

    /// Flip board to `quarantined` and record a reason. Returns
    /// `DbError::NotFound` if `id` does not exist.
    pub fn quarantine(conn: &Connection, id: &str, reason: &str) -> Result<()> {
        let n = conn.execute(
            "UPDATE board SET health = 'quarantined', quarantine_reason = ?1
             WHERE id = ?2",
            params![reason, id],
        )?;
        require_one_row(n, id)
    }

    /// Flip board back to `healthy`, clear quarantine reason and reset the
    /// infra failure counter. Returns `DbError::NotFound` if `id` does
    /// not exist.
    pub fn unquarantine(conn: &Connection, id: &str) -> Result<()> {
        let n = conn.execute(
            "UPDATE board SET health = 'healthy', quarantine_reason = NULL,
             consecutive_infra_failures = 0 WHERE id = ?1",
            params![id],
        )?;
        require_one_row(n, id)
    }

    /// Permanently delete a board row. Refused unless the row is
    /// currently quarantined. Refused if any job row references this
    /// board_id (SQLite FK with PRAGMA foreign_keys = ON enforces this).
    ///
    /// Error mapping:
    /// - `DbError::NotFound { entity: "board", .. }` if the id is unknown
    /// - `DbError::Conflict { entity: "board", reason: "...quarantined first...", .. }`
    ///   if the row is currently healthy
    /// - `DbError::Conflict { entity: "board", reason: "referenced by N job row(s)...", .. }`
    ///   if at least one job row references this board
    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        let row = match Self::find(conn, id)? {
            Some(r) => r,
            None => {
                return Err(DbError::NotFound {
                    entity: "board",
                    id: id.to_string(),
                });
            }
        };
        if row.spec.health != BoardHealth::Quarantined {
            return Err(DbError::Conflict {
                entity: "board",
                id: id.to_string(),
                reason: "board must be quarantined first; use POST /boards/:id/quarantine".into(),
            });
        }
        let result = conn.execute("DELETE FROM board WHERE id = ?1", params![id]);
        match result {
            Ok(n) => {
                if n == 0 {
                    Err(DbError::NotFound {
                        entity: "board",
                        id: id.to_string(),
                    })
                } else {
                    Ok(())
                }
            }
            Err(RusqliteError::SqliteFailure(e, _)) if e.code == ErrorCode::ConstraintViolation => {
                // Best-effort: count the offenders so the operator
                // knows the scale. If the count query itself fails for
                // any reason, fall back to a generic message — the
                // outer Conflict is still useful.
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM job WHERE board_id = ?1",
                        params![id],
                        |r| r.get(0),
                    )
                    .unwrap_or(-1);
                let reason = if count >= 0 {
                    format!("referenced by {count} job row(s); wait for retention to age them out")
                } else {
                    "referenced by existing job rows; wait for retention to age them out"
                        .to_string()
                };
                Err(DbError::Conflict {
                    entity: "board",
                    id: id.to_string(),
                    reason,
                })
            }
            Err(other) => Err(other.into()),
        }
    }
}

/// Map a `rusqlite::execute` rows-affected count to `Ok(())` or
/// `Err(DbError::NotFound { entity: "board", id })`. Used by every
/// single-row board mutator to turn silent no-ops into typed errors.
fn require_one_row(n: usize, id: &str) -> Result<()> {
    if n == 0 {
        Err(DbError::NotFound {
            entity: "board",
            id: id.to_string(),
        })
    } else {
        Ok(())
    }
}

/// Escape `LIKE` wildcards in a user-supplied substring so `%`, `_`, and
/// `\` are matched literally (paired with `ESCAPE '\'` in the query). Used
/// by [`BoardRow::list_page`] / [`BoardRow::count`] so a filter like `a_b`
/// matches the literal characters rather than `_` standing for any char.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '%' || c == '_' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
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
