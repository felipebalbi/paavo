//! Aggregate counts and the consolidated payload for the dashboard
//! landing page. Pure data; computed server-side from SQL aggregates so
//! the client never counts rows.
use crate::{BoardView, JobListItem};
use serde::{Deserialize, Serialize};

/// All-time job counts by state, over retained rows. The dashboard's
/// derived tallies (queue depth, terminal total, pass rate) are computed
/// from these via the helpers below, so "what counts as queued /
/// terminal" has exactly one definition shared by every consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobStateCounts {
    /// Accepted, not yet dispatched.
    pub submitted: u64,
    /// Compiling.
    pub building: u64,
    /// Built; waiting for a free matching board.
    pub awaiting_board: u64,
    /// Attached to a probe.
    pub running: u64,
    /// Terminal: `Test OK` + bkpt.
    pub passed: u64,
    /// Terminal: build / test / infra error.
    pub failed: u64,
    /// Terminal: inactivity or hard-max watchdog.
    pub timed_out: u64,
    /// Terminal: user cancel / daemon shutdown / interrupted.
    pub aborted: u64,
}

impl JobStateCounts {
    /// Jobs accepted but not yet running: submitted + building + awaiting_board.
    pub fn queue(&self) -> u64 {
        self.submitted + self.building + self.awaiting_board
    }

    /// Jobs in a terminal state: passed + failed + timed_out + aborted.
    pub fn terminal(&self) -> u64 {
        self.passed + self.failed + self.timed_out + self.aborted
    }

    /// Whole-percent pass rate over terminal jobs, or `None` when there
    /// are no terminal jobs yet (the card renders "—").
    pub fn pass_rate_pct(&self) -> Option<u64> {
        let t = self.terminal();
        (t > 0).then(|| (self.passed as f64 / t as f64 * 100.0).round() as u64)
    }
}

/// Board fleet health tally. `health` has only two values, so healthy is
/// derived rather than transmitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardHealthCounts {
    /// All registered boards.
    pub total: u64,
    /// Boards currently quarantined.
    pub quarantined: u64,
}

impl BoardHealthCounts {
    /// total - quarantined (saturating; the two are always consistent in
    /// a single snapshot, but saturating keeps the type total-correct).
    pub fn healthy(&self) -> u64 {
        self.total.saturating_sub(self.quarantined)
    }
}

/// One-shot payload backing the dashboard landing page: exact aggregate
/// counts plus the two short display lists the page renders. Fully
/// bounded — its size does not grow with the fleet or job history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardOverview {
    /// All-time job counts by state.
    pub jobs: JobStateCounts,
    /// Board fleet health tally.
    pub boards: BoardHealthCounts,
    /// Newest-first; capped (default 8) for the "Recent activity" table.
    pub recent_jobs: Vec<JobListItem>,
    /// Quarantined-first then most-recently-used; capped (default 8) for
    /// the "Board fleet" table.
    pub fleet: Vec<BoardView>,
    /// Jobs resource revision at query time (echoed for live de-dup / debug).
    pub jobs_revision: u64,
    /// Boards resource revision at query time.
    pub boards_revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BoardHealth, BoardSpec, JobId, JobState, Priority, ProbeSelector};

    fn counts() -> JobStateCounts {
        JobStateCounts {
            submitted: 1,
            building: 2,
            awaiting_board: 3,
            running: 4,
            passed: 6,
            failed: 2,
            timed_out: 1,
            aborted: 1,
        }
    }

    #[test]
    fn job_state_counts_roundtrips() {
        let c = counts();
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(c, serde_json::from_str::<JobStateCounts>(&j).unwrap());
    }

    #[test]
    fn queue_and_terminal_sum_the_right_buckets() {
        let c = counts();
        assert_eq!(c.queue(), 1 + 2 + 3);
        assert_eq!(c.terminal(), 6 + 2 + 1 + 1);
    }

    #[test]
    fn pass_rate_rounds_over_terminal() {
        // 6 passed of 10 terminal => 60% (exact).
        assert_eq!(counts().pass_rate_pct(), Some(60));
        // 2 of 3 terminal => 66.66.. rounds up to 67 (truncation would give 66).
        let two_thirds = JobStateCounts {
            submitted: 0,
            building: 0,
            awaiting_board: 0,
            running: 0,
            passed: 2,
            failed: 1,
            timed_out: 0,
            aborted: 0,
        };
        assert_eq!(two_thirds.pass_rate_pct(), Some(67));
        // 1 of 8 terminal => 12.5 pins the round-half-away-from-zero boundary.
        let one_eighth = JobStateCounts {
            submitted: 0,
            building: 0,
            awaiting_board: 0,
            running: 0,
            passed: 1,
            failed: 7,
            timed_out: 0,
            aborted: 0,
        };
        assert_eq!(one_eighth.pass_rate_pct(), Some(13));
    }

    #[test]
    fn pass_rate_is_none_with_no_terminal_jobs() {
        let c = JobStateCounts {
            submitted: 5,
            building: 0,
            awaiting_board: 0,
            running: 0,
            passed: 0,
            failed: 0,
            timed_out: 0,
            aborted: 0,
        };
        assert_eq!(c.terminal(), 0);
        assert_eq!(c.pass_rate_pct(), None);
    }

    #[test]
    fn board_health_counts_roundtrips() {
        let c = BoardHealthCounts {
            total: 9,
            quarantined: 2,
        };
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(c, serde_json::from_str::<BoardHealthCounts>(&j).unwrap());
    }

    #[test]
    fn healthy_is_total_minus_quarantined() {
        assert_eq!(
            BoardHealthCounts {
                total: 9,
                quarantined: 2
            }
            .healthy(),
            7
        );
        // Saturating: never underflows even on an inconsistent pair.
        assert_eq!(
            BoardHealthCounts {
                total: 0,
                quarantined: 3
            }
            .healthy(),
            0
        );
    }

    #[test]
    fn dashboard_overview_roundtrips() {
        let job = JobListItem {
            id: JobId::new(),
            state: JobState::Running,
            priority: Priority::Interactive,
            submitter: "alice".into(),
            board_id: Some("mcxa266-01".into()),
            submitted_at: 1_700_000_000_000,
        };
        let board = BoardView {
            spec: BoardSpec {
                id: "mcxa266-01".into(),
                kind: "mcxa266".into(),
                probe_selector: ProbeSelector {
                    vid: "1366".into(),
                    pid: "1015".into(),
                    serial: "ABC".into(),
                    interface: None,
                },
                chip_name: "MCXA266VFL".into(),
                target_name: "frdm-mcx-a266".into(),
                wiring_profile: Some("default".into()),
                health: BoardHealth::Healthy,
            },
            quarantine_reason: None,
            consecutive_infra_failures: 0,
            last_used_at: Some(1_700_000_000_000),
            created_at: 1_699_000_000_000,
        };
        let over = DashboardOverview {
            jobs: counts(),
            boards: BoardHealthCounts {
                total: 4,
                quarantined: 1,
            },
            recent_jobs: vec![job],
            fleet: vec![board],
            jobs_revision: 7,
            boards_revision: 3,
        };
        let j = serde_json::to_string(&over).unwrap();
        assert_eq!(over, serde_json::from_str::<DashboardOverview>(&j).unwrap());
    }
}
