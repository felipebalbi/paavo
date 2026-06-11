//! Cancellation path that lives entirely in paavo-core: short-circuit for
//! the `Submitted` state. The `Building`/`Running` paths go through paavod
//! because they need to signal a running BoardWorker — that wiring lives in
//! M4.

use crate::error::{CoreError, Result};
use paavo_proto::{AbortReason, JobId, JobOutcome, JobState};
use rusqlite::Connection;

/// If the job is in `Submitted` state, mark it `Aborted{User}` and return
/// the outcome. Otherwise, return `NotCancellable`.
///
/// `now_ms` is the wall-clock instant to record as `finished_at_ms`.
/// Production passes `Utc::now().timestamp_millis()`; tests inject
/// deterministic values.
///
/// Returns `Some(JobOutcome::Aborted)` when the inline finalization
/// succeeds. The `Option` shape is preserved so M4's wrapping function
/// (which signals a BoardWorker async-style) can return `Ok(None)` to
/// mean "cancel-signal sent, finalize will happen via worker_done".
///
/// **Single-writer assumption.** The `get → finalize` pair is two SQL
/// statements; concurrent writers could race. paavod's dispatch thread is
/// single-threaded, so in production there is exactly one writer here.
/// The `JobRow::finalize`'s `WHERE state IN ('submitted','building',
/// 'running')` guard catches a racing terminal transition (the racing
/// writer would have left the row in `Aborted`/`Passed`/etc., and our
/// `finalize` would no-op-and-error out via the `n == 0` branch in
/// paavo-db rather than silently return `Ok(Some(Aborted))` over a row
/// the racing writer already changed).
pub fn cancel_if_submitted(
    conn: &Connection,
    id: &JobId,
    now_ms: i64,
) -> Result<Option<JobOutcome>> {
    let row = paavo_db::JobRow::get(conn, id)?;
    if row.state != JobState::Submitted {
        return Err(CoreError::NotCancellable { state: row.state });
    }
    let outcome = JobOutcome::Aborted {
        by: AbortReason::User,
    };
    paavo_db::JobRow::finalize(
        conn,
        id,
        &paavo_db::OutcomeRecord {
            state: JobState::Aborted,
            outcome: outcome.clone(),
            finished_at_ms: now_ms,
        },
    )?;
    Ok(Some(outcome))
}
