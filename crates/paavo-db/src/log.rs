//! Log frame table typed helpers, plus the truncate-on-pass retention sweep.

use crate::error::{DbError, Result};
use paavo_proto::{JobId, LogFrame, LogLevel};
use rusqlite::{params, Connection, Row};

/// Type alias for `paavo_proto::LogFrame`. Re-exported via `paavo_db::LogFrameRow`
/// so callers can write `use paavo_db::{LogFrameRow, LogFrameDb};` without
/// also importing from `paavo_proto`. Consistent in spirit with `BoardRow` /
/// `JobRow` even though those are full struct definitions because they carry
/// DB-only fields beyond the proto type.
pub type LogFrameRow = LogFrame;

/// Extension trait providing `log_frame` table operations on `LogFrame`.
///
/// `LogFrame` lives in `paavo-proto`, so we cannot add inherent methods to
/// it here without violating Rust's orphan rules. The trait gives callers
/// the same `LogFrame::append_batch(...)` call-site ergonomics — they just
/// need `use paavo_db::LogFrameDb;` in scope.
pub trait LogFrameDb: Sized {
    /// Append a batch of frames for a job in one transaction. An empty
    /// slice is a no-op (an empty transaction commits successfully).
    fn append_batch(conn: &Connection, job_id: &JobId, frames: &[Self]) -> Result<()>;
    /// Return frames `[offset, offset+limit)` ordered by `seq` ascending.
    fn list(conn: &Connection, job_id: &JobId, offset: u32, limit: u32) -> Result<Vec<Self>>;
    /// Total frame count for a job.
    fn count_for_job(conn: &Connection, job_id: &JobId) -> Result<u64>;
    /// Retention sweep. Delete frames with `level IN (trace, debug, info)`
    /// for any `Passed` job whose `finished_at` is older than
    /// `passed_full_log_days` ago. Warn and error frames are kept
    /// indefinitely (spec §7.6).
    ///
    /// `passed_full_log_days < 0` disables truncation entirely. Returns
    /// the number of frames deleted.
    fn truncate_old_passed(
        conn: &Connection,
        passed_full_log_days: i32,
        now_ms: i64,
    ) -> Result<u64>;
}

impl LogFrameDb for LogFrame {
    fn append_batch(conn: &Connection, job_id: &JobId, frames: &[Self]) -> Result<()> {
        // `unchecked_transaction` takes `&Connection` (not `&mut`); safe here
        // because paavod is the single writer per spec §7.
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO log_frame (job_id, seq, ts_us, level, target, message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for f in frames {
                stmt.execute(params![
                    job_id.to_string(),
                    f.seq as i64,
                    f.ts_us as i64,
                    level_to_str(f.level),
                    f.target,
                    f.message,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn list(conn: &Connection, job_id: &JobId, offset: u32, limit: u32) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT seq, ts_us, level, target, message FROM log_frame
             WHERE job_id = ?1 ORDER BY seq ASC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt
            .query_map(
                params![job_id.to_string(), limit as i64, offset as i64],
                row_to_frame,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn count_for_job(conn: &Connection, job_id: &JobId) -> Result<u64> {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM log_frame WHERE job_id = ?1",
            params![job_id.to_string()],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }

    fn truncate_old_passed(
        conn: &Connection,
        passed_full_log_days: i32,
        now_ms: i64,
    ) -> Result<u64> {
        if passed_full_log_days < 0 {
            return Ok(0);
        }
        let cutoff = now_ms - i64::from(passed_full_log_days) * 86_400_000;
        let n = conn.execute(
            "DELETE FROM log_frame
             WHERE level IN ('trace','debug','info')
               AND job_id IN (
                   SELECT id FROM job
                   WHERE state = 'passed'
                     AND finished_at IS NOT NULL
                     AND finished_at < ?1
               )",
            params![cutoff],
        )?;
        Ok(n as u64)
    }
}

fn level_to_str(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

fn level_from_str(s: &str) -> Result<LogLevel> {
    Ok(match s {
        "trace" => LogLevel::Trace,
        "debug" => LogLevel::Debug,
        "info" => LogLevel::Info,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        other => {
            return Err(DbError::UnknownEnum {
                column: "log_frame.level",
                value: other.to_string(),
            })
        }
    })
}

fn row_to_frame(r: &Row<'_>) -> rusqlite::Result<Result<LogFrame>> {
    let seq_raw: i64 = r.get("seq")?;
    let ts_us_raw: i64 = r.get("ts_us")?;
    let level_str: String = r.get("level")?;
    let target: Option<String> = r.get("target")?;
    let message: String = r.get("message")?;
    Ok((|| -> Result<LogFrame> {
        let level = level_from_str(&level_str)?;
        let seq: u64 = seq_raw.try_into().map_err(|_| DbError::UnknownEnum {
            column: "log_frame.seq",
            value: "negative".into(),
        })?;
        let ts_us: u64 = ts_us_raw.try_into().map_err(|_| DbError::UnknownEnum {
            column: "log_frame.ts_us",
            value: "negative".into(),
        })?;
        Ok(LogFrame {
            seq,
            ts_us,
            level,
            target,
            message,
        })
    })())
}
