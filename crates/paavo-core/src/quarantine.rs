//! Quarantine policy. Reacts to terminal outcomes.

use paavo_proto::JobOutcome;
use rusqlite::Connection;

/// Policy parameters (from `paavo.toml::[quarantine]`).
#[derive(Debug, Clone, Copy)]
pub struct QuarantinePolicy {
    /// Threshold: when `consecutive_infra_failures` reaches this, the
    /// board is auto-quarantined with reason
    /// `"auto: N consecutive infra failures"`.
    pub consecutive_infra_failures: u32,
}

impl Default for QuarantinePolicy {
    fn default() -> Self {
        Self {
            consecutive_infra_failures: 3,
        }
    }
}

/// Format the auto-quarantine reason string for a board that has reached
/// `consecutive_infra_failures` infra failures. Stable so the web UI (per
/// spec §11) and tests can refer to the same text.
pub fn auto_quarantine_reason(n: u32) -> String {
    format!("auto: {n} consecutive infra failures")
}

/// Apply an outcome to a board's quarantine state. Caller must have already
/// called `JobRow::finalize`. Returns `Ok(true)` if the board was just
/// auto-quarantined.
///
/// **Single-writer assumption.** The bump-then-get pair is two SQL
/// statements; concurrent calls could race. paavod's dispatch thread is
/// single-threaded, so in production there is exactly one writer here.
pub fn apply_outcome_to_board(
    conn: &Connection,
    board_id: &str,
    outcome: &JobOutcome,
    probe_released_cleanly: bool,
    policy: QuarantinePolicy,
) -> paavo_db::Result<bool> {
    // The "outcome → infra-failure?" rule is split across two layers per
    // proto's own doc-comment: `JobOutcome::counts_toward_infra_failure`
    // owns the outcome-only part, and we add the "probe didn't release on
    // an inactivity timeout" predicate that paavo-proto can't see.
    let counts_toward_infra = outcome.counts_toward_infra_failure()
        || (matches!(
            outcome,
            JobOutcome::TimedOut {
                reason: paavo_proto::TimeoutReason::Inactivity,
                ..
            }
        ) && !probe_released_cleanly);
    if !counts_toward_infra {
        paavo_db::BoardRow::reset_infra_failures(conn, board_id)?;
        return Ok(false);
    }
    paavo_db::BoardRow::bump_infra_failure(conn, board_id)?;
    let row = paavo_db::BoardRow::get(conn, board_id)?;
    if row.consecutive_infra_failures >= policy.consecutive_infra_failures {
        paavo_db::BoardRow::quarantine(
            conn,
            board_id,
            &auto_quarantine_reason(row.consecutive_infra_failures),
        )?;
        return Ok(true);
    }
    Ok(false)
}
