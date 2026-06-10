//! Enqueue path: validate selector & ceiling, persist tar metadata, insert
//! job row in `Submitted` state.

use crate::error::{CoreError, Result};
use crate::selector::selector_matches_any;
use paavo_proto::{BoardSelector, BoardSpec, JobId, JobSource, Priority};
use rusqlite::Connection;

/// One enqueue request.
#[derive(Debug, Clone)]
pub struct EnqueueRequest {
    /// Pre-allocated job id (caller may want to log it before insert).
    pub job_id: JobId,
    /// Scheduler priority.
    pub priority: Priority,
    /// Submitter free text.
    pub submitter: String,
    /// Where the request came from.
    pub source: JobSource,
    /// Selector.
    pub board_selector: BoardSelector,
    /// Effective inactivity ms (already resolved against the daemon default
    /// by the HTTP layer; ELF override is applied later).
    pub inactivity_timeout_ms: u64,
    /// Effective hard-max ms.
    pub hard_max_ms: u64,
    /// blake3 of the uploaded tar.
    pub tar_blake3: String,
    /// On-disk persisted tar path.
    pub tar_path: String,
    /// Daemon ceiling for hard-max; requests above this are rejected.
    pub daemon_ceiling_ms: u64,
}

/// Validate + persist a new job. `now_ms` is the wall-clock instant
/// recorded as the job's `submitted_at` — production passes
/// `Utc::now().timestamp_millis()`, tests inject deterministic values.
pub fn enqueue_job(
    conn: &Connection,
    inventory: &[BoardSpec],
    req: EnqueueRequest,
    now_ms: i64,
) -> Result<JobId> {
    if req.hard_max_ms > req.daemon_ceiling_ms {
        return Err(CoreError::OverCeiling {
            requested: req.hard_max_ms,
            ceiling: req.daemon_ceiling_ms,
        });
    }
    if !selector_matches_any(&req.board_selector, inventory) {
        return Err(CoreError::SelectorNeverMatches(req.board_selector));
    }
    let new = paavo_db::NewJob {
        id: req.job_id,
        priority: req.priority,
        submitter: req.submitter,
        source: req.source,
        board_selector: req.board_selector,
        inactivity_timeout_ms: req.inactivity_timeout_ms,
        hard_max_ms: req.hard_max_ms,
        tar_blake3: req.tar_blake3,
        tar_path: req.tar_path,
    };
    paavo_db::JobRow::insert(conn, &new, now_ms)?;
    Ok(req.job_id)
}
