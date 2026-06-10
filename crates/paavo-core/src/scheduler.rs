//! Scheduler: pick the highest-priority eligible job + LRU healthy board.

use paavo_proto::Priority;
use rusqlite::Connection;

/// Maximum number of `Submitted` jobs `pick_next` scans per call. Safe to
/// truncate because `JobRow::list_submitted` already returns rows in
/// `(priority ASC, submitted_at ASC)` order, so any job beyond this limit
/// cannot out-rank the rows we already have.
const MAX_SUBMITTED_SCAN: u32 = 200;

/// Scheduler configuration (subset of `paavo.toml`).
#[derive(Debug, Clone, Copy)]
pub struct SchedulerConfig {
    /// Scheduled jobs older than this get promoted to Interactive priority
    /// (spec §5.3 starvation rule).
    pub starvation_threshold_ms: i64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            starvation_threshold_ms: 6 * 60 * 60 * 1_000, // 6 h
        }
    }
}

/// A successful pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledJob {
    /// Job to dispatch.
    pub job: paavo_db::JobRow,
    /// Board to dispatch onto.
    pub board: paavo_db::BoardRow,
}

/// Look at all `Submitted` jobs in priority/starvation order; for each, look
/// at all eligible healthy boards in LRU order (`last_used_at ASC NULLS
/// FIRST`); return the first matching pair, or `Ok(None)` if nothing can
/// dispatch right now.
///
/// `now_ms` is the wall-clock instant the scheduler should treat as "now"
/// for starvation-promotion purposes — production passes
/// `Utc::now().timestamp_millis()`, tests inject deterministic values.
///
/// Pure read — caller is responsible for the subsequent
/// `JobRow::transition_to_building` + `BoardRow::touch_last_used`.
pub fn pick_next(
    conn: &Connection,
    config: SchedulerConfig,
    now_ms: i64,
) -> paavo_db::Result<Option<ScheduledJob>> {
    let jobs = paavo_db::JobRow::list_submitted(conn, MAX_SUBMITTED_SCAN)?;
    // Promote scheduled jobs that have starved.
    let mut promoted: Vec<paavo_db::JobRow> = jobs
        .into_iter()
        .map(|mut j| {
            if j.priority == Priority::Scheduled
                && now_ms - j.submitted_at >= config.starvation_threshold_ms
            {
                j.priority = Priority::Interactive;
            }
            j
        })
        .collect();
    promoted.sort_by_key(|j| (j.priority.weight(), j.submitted_at));

    for job in promoted {
        let boards = paavo_db::BoardRow::find_healthy_for_selector(conn, &job.board_selector)?;
        if let Some(pick) = lru_pick(boards) {
            return Ok(Some(ScheduledJob { job, board: pick }));
        }
    }
    Ok(None)
}

/// Sort by LRU (`None` first, then ascending `last_used_at`, ties broken by
/// `spec.id`) and return the head, or `None` if `boards` is empty.
fn lru_pick(mut boards: Vec<paavo_db::BoardRow>) -> Option<paavo_db::BoardRow> {
    // Sort: never-used (None) first, then ascending last_used_at; then id
    // ascending for determinism.
    boards.sort_by(|a, b| match (a.last_used_at, b.last_used_at) {
        (None, None) => a.spec.id.cmp(&b.spec.id),
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(x), Some(y)) => x.cmp(&y).then(a.spec.id.cmp(&b.spec.id)),
    });
    boards.into_iter().next()
}
