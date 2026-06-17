# Dashboard SQL Aggregates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the dashboard's stat tallies off client-side WASM counting and into SQL aggregates, served via one consolidated `GET /api/dashboard` endpoint, so the numbers stay exact and the page stays snappy at any scale.

**Architecture:** New `paavo-proto` wire types (`JobStateCounts`, `BoardHealthCounts`, `DashboardOverview`) carry exact counts plus two short display lists. New `paavo-db` aggregate queries (`JobRow::state_counts`, `BoardRow::health_counts`, `BoardRow::list_dashboard`) back them. A new `paavo-web` handler assembles the payload (counts + fleet slice from SQL, 8 recent jobs from the in-memory index). The Leptos dashboard fetches that one payload and only renders. Changes flow strictly up the dependency DAG (proto → db → web → web-ui); no migration.

**Tech Stack:** Rust 1.95, rusqlite/SQLite (WAL), axum, serde, Leptos CSR (wasm32), `tempfile` + `tower::ServiceExt` test harness.

**Reference spec:** `docs/superpowers/specs/2026-06-17-dashboard-sql-aggregates-design.md`

---

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/paavo-proto/src/stats.rs` | **New.** The three stats wire types + derived helpers + serde/unit tests. |
| `crates/paavo-proto/src/lib.rs` | Wire `mod stats;` + re-export the three types. |
| `crates/paavo-db/src/job.rs` | Add `JobRow::state_counts`. |
| `crates/paavo-db/src/board.rs` | Add `BoardRow::health_counts` + `BoardRow::list_dashboard`. |
| `crates/paavo-db/tests/job_ops.rs` | Tests for `state_counts`. |
| `crates/paavo-db/tests/board_ops.rs` | Tests for `health_counts` + `list_dashboard`. |
| `crates/paavo-web/src/db.rs` | Three RO façade wrappers over the new db queries. |
| `crates/paavo-web/src/api/dashboard.rs` | **New.** `GET /api/dashboard` handler. |
| `crates/paavo-web/src/api/boards.rs` | Make `board_view` `pub(crate)` for reuse. |
| `crates/paavo-web/src/api/mod.rs` | `pub mod dashboard;`. |
| `crates/paavo-web/src/app.rs` | Register the `/api/dashboard` route. |
| `crates/paavo-web/tests/api_dashboard.rs` | **New.** Integration test for the endpoint. |
| `crates/paavo-web-ui/src/api.rs` | `dashboard()` fetch wrapper. |
| `crates/paavo-web-ui/src/components/dashboard.rs` | Rewrite to one resource that reads aggregates. |

---

## Task 1: `paavo-proto` — `JobStateCounts` + derived helpers

**Files:**
- Create: `crates/paavo-proto/src/stats.rs`
- Modify: `crates/paavo-proto/src/lib.rs`

- [ ] **Step 1: Create `stats.rs` with the type, helpers, and tests**

Create `crates/paavo-proto/src/stats.rs` with exactly:

```rust
//! Aggregate counts and the consolidated payload for the dashboard
//! landing page. Pure data; computed server-side from SQL aggregates so
//! the client never counts rows.
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // 6 passed of 10 terminal => 60%.
        assert_eq!(counts().pass_rate_pct(), Some(60));
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
}
```

- [ ] **Step 2: Wire the module into `lib.rs`**

In `crates/paavo-proto/src/lib.rs`, add `mod stats;` to the module list (after `mod schedule;`):

```rust
mod board;
mod ids;
mod job;
mod log;
mod page;
mod schedule;
mod stats;
mod stream;
```

And add the re-export (after the `pub use schedule::ScheduleView;` line). Re-export only the type that exists after this task — Tasks 2 and 3 extend this same line as they add the other two types, so each task's build stays green:

```rust
pub use stats::JobStateCounts;
```

- [ ] **Step 3: Run the proto tests to verify they pass**

Run: `cargo test -p paavo-proto stats`
Expected: PASS (4 tests in the `stats::tests` module).

- [ ] **Step 4: Commit**

```bash
git add crates/paavo-proto/src/stats.rs crates/paavo-proto/src/lib.rs
git commit -m "feat(proto): JobStateCounts with queue/terminal/pass-rate helpers"
```

---

## Task 2: `paavo-proto` — `BoardHealthCounts`

**Files:**
- Modify: `crates/paavo-proto/src/stats.rs`
- Modify: `crates/paavo-proto/src/lib.rs`

- [ ] **Step 1: Append the type + helper + tests to `stats.rs`**

Add after the `JobStateCounts` `impl` block (and before the `#[cfg(test)] mod tests`):

```rust
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
```

Add these tests inside the existing `mod tests` block:

```rust
    #[test]
    fn board_health_counts_roundtrips() {
        let c = BoardHealthCounts { total: 9, quarantined: 2 };
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(c, serde_json::from_str::<BoardHealthCounts>(&j).unwrap());
    }

    #[test]
    fn healthy_is_total_minus_quarantined() {
        assert_eq!(BoardHealthCounts { total: 9, quarantined: 2 }.healthy(), 7);
        // Saturating: never underflows even on an inconsistent pair.
        assert_eq!(BoardHealthCounts { total: 0, quarantined: 3 }.healthy(), 0);
    }
```

- [ ] **Step 2: Extend the re-export in `lib.rs`**

Change the stats re-export line to:

```rust
pub use stats::{BoardHealthCounts, JobStateCounts};
```

- [ ] **Step 3: Run the proto tests**

Run: `cargo test -p paavo-proto stats`
Expected: PASS (now 6 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/paavo-proto/src/stats.rs crates/paavo-proto/src/lib.rs
git commit -m "feat(proto): BoardHealthCounts with derived healthy()"
```

---

## Task 3: `paavo-proto` — `DashboardOverview`

**Files:**
- Modify: `crates/paavo-proto/src/stats.rs`
- Modify: `crates/paavo-proto/src/lib.rs`

- [ ] **Step 1: Add the `crate` imports at the top of `stats.rs`**

Change the import block at the top of `stats.rs` to:

```rust
use crate::{BoardView, JobListItem};
use serde::{Deserialize, Serialize};
```

- [ ] **Step 2: Append `DashboardOverview` to `stats.rs`**

Add after the `BoardHealthCounts` `impl` block (before `#[cfg(test)] mod tests`):

```rust
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
```

- [ ] **Step 3: Add a round-trip test inside `mod tests`**

The test needs `JobListItem` / `BoardView` constructors. `BoardView` and `JobListItem` are already in scope via `use super::*;` (the module re-exports them at its top), so import only the additional names. Add this `use` inside the `mod tests` block (after `use super::*;`):

```rust
    use crate::{BoardHealth, BoardSpec, JobId, JobState, Priority, ProbeSelector};
```

Then add the test:

```rust
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
            boards: BoardHealthCounts { total: 4, quarantined: 1 },
            recent_jobs: vec![job],
            fleet: vec![board],
            jobs_revision: 7,
            boards_revision: 3,
        };
        let j = serde_json::to_string(&over).unwrap();
        assert_eq!(over, serde_json::from_str::<DashboardOverview>(&j).unwrap());
    }
```

- [ ] **Step 4: Finalize the re-export in `lib.rs`**

Change the stats re-export line to:

```rust
pub use stats::{BoardHealthCounts, DashboardOverview, JobStateCounts};
```

- [ ] **Step 5: Run the proto tests**

Run: `cargo test -p paavo-proto stats`
Expected: PASS (now 7 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/paavo-proto/src/stats.rs crates/paavo-proto/src/lib.rs
git commit -m "feat(proto): DashboardOverview consolidated landing-page payload"
```

---

## Task 4: `paavo-db` — `JobRow::state_counts`

**Files:**
- Modify: `crates/paavo-db/src/job.rs`
- Test: `crates/paavo-db/tests/job_ops.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/paavo-db/tests/job_ops.rs`:

```rust
#[test]
fn state_counts_empty_db_all_zero() {
    let db = fresh_db();
    let c = JobRow::state_counts(db.raw_conn()).unwrap();
    assert_eq!(c.submitted, 0);
    assert_eq!(c.running, 0);
    assert_eq!(c.terminal(), 0);
    assert_eq!(c.queue(), 0);
}

#[test]
fn state_counts_tallies_each_state() {
    let db = fresh_db();
    insert_default_board(&db);
    let now = Utc::now().timestamp_millis();

    // Two submitted (left as-is).
    JobRow::insert(db.raw_conn(), &sample_new_job(JobId::new()), now).unwrap();
    JobRow::insert(db.raw_conn(), &sample_new_job(JobId::new()), now).unwrap();

    // One building.
    let b = JobId::new();
    JobRow::insert(db.raw_conn(), &sample_new_job(b), now).unwrap();
    JobRow::transition_submitted_to_building(db.raw_conn(), &b, now + 1).unwrap();

    // One passed (full lifecycle on the seeded board).
    let p = JobId::new();
    JobRow::insert(db.raw_conn(), &sample_new_job(p), now).unwrap();
    JobRow::transition_submitted_to_building(db.raw_conn(), &p, now + 1).unwrap();
    JobRow::transition_building_to_awaiting_board(db.raw_conn(), &p, "/elf").unwrap();
    JobRow::transition_awaiting_to_running(db.raw_conn(), &p, "mcxa266-01").unwrap();
    JobRow::finalize(
        db.raw_conn(),
        &p,
        &OutcomeRecord {
            state: JobState::Passed,
            outcome: JobOutcome::Passed,
            finished_at_ms: now + 2,
        },
    )
    .unwrap();

    // One failed (test error).
    let f = JobId::new();
    JobRow::insert(db.raw_conn(), &sample_new_job(f), now).unwrap();
    JobRow::transition_submitted_to_building(db.raw_conn(), &f, now + 1).unwrap();
    JobRow::transition_building_to_awaiting_board(db.raw_conn(), &f, "/elf").unwrap();
    JobRow::transition_awaiting_to_running(db.raw_conn(), &f, "mcxa266-01").unwrap();
    JobRow::finalize(
        db.raw_conn(),
        &f,
        &OutcomeRecord {
            state: JobState::Failed,
            outcome: JobOutcome::Failed(TerminalOutcome::TestErr { message: "boom".into() }),
            finished_at_ms: now + 2,
        },
    )
    .unwrap();

    let c = JobRow::state_counts(db.raw_conn()).unwrap();
    assert_eq!(c.submitted, 2);
    assert_eq!(c.building, 1);
    assert_eq!(c.awaiting_board, 0);
    assert_eq!(c.running, 0);
    assert_eq!(c.passed, 1);
    assert_eq!(c.failed, 1);
    assert_eq!(c.timed_out, 0);
    assert_eq!(c.aborted, 0);
    assert_eq!(c.queue(), 3);
    assert_eq!(c.terminal(), 2);
}
```

(The imports `JobOutcome`, `JobState`, `TerminalOutcome`, `OutcomeRecord` are already present at the top of `job_ops.rs`.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p paavo-db --test job_ops state_counts`
Expected: FAIL to compile — `no function or associated item named state_counts found for struct JobRow`.

- [ ] **Step 3: Implement `state_counts` in `job.rs`**

Add this method inside `impl JobRow` (e.g. right after `count`):

```rust
    /// Job counts grouped by state, all-time over retained rows. Backed
    /// by `idx_job_state` (SQLite satisfies the GROUP BY from the index).
    /// Unknown state strings surface as `DbError::UnknownEnum`, matching
    /// the `from_row` family. States with no rows stay zero.
    pub fn state_counts(conn: &Connection) -> Result<paavo_proto::JobStateCounts> {
        let mut counts = paavo_proto::JobStateCounts {
            submitted: 0,
            building: 0,
            awaiting_board: 0,
            running: 0,
            passed: 0,
            failed: 0,
            timed_out: 0,
            aborted: 0,
        };
        let mut stmt = conn.prepare("SELECT state, COUNT(*) FROM job GROUP BY state")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (state_str, n) = row?;
            let n = n as u64;
            match state_from_str(&state_str)? {
                JobState::Submitted => counts.submitted = n,
                JobState::Building => counts.building = n,
                JobState::AwaitingBoard => counts.awaiting_board = n,
                JobState::Running => counts.running = n,
                JobState::Passed => counts.passed = n,
                JobState::Failed => counts.failed = n,
                JobState::TimedOut => counts.timed_out = n,
                JobState::Aborted => counts.aborted = n,
            }
        }
        Ok(counts)
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p paavo-db --test job_ops state_counts`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-db/src/job.rs crates/paavo-db/tests/job_ops.rs
git commit -m "feat(db): JobRow::state_counts — GROUP BY state aggregate"
```

---

## Task 5: `paavo-db` — `BoardRow::health_counts`

**Files:**
- Modify: `crates/paavo-db/src/board.rs`
- Test: `crates/paavo-db/tests/board_ops.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/paavo-db/tests/board_ops.rs`:

```rust
#[test]
fn health_counts_empty_db_is_zero() {
    let db = fresh_db();
    let c = BoardRow::health_counts(db.raw_conn()).unwrap();
    assert_eq!(c.total, 0);
    assert_eq!(c.quarantined, 0);
    assert_eq!(c.healthy(), 0);
}

#[test]
fn health_counts_totals_and_quarantined() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    for id in ["b1", "b2", "b3"] {
        let mut s = sample_board();
        s.id = id.into();
        BoardRow::insert(db.raw_conn(), &s, now).unwrap();
    }
    BoardRow::quarantine(db.raw_conn(), "b2", "broken").unwrap();

    let c = BoardRow::health_counts(db.raw_conn()).unwrap();
    assert_eq!(c.total, 3);
    assert_eq!(c.quarantined, 1);
    assert_eq!(c.healthy(), 2);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p paavo-db --test board_ops health_counts`
Expected: FAIL to compile — `no function or associated item named health_counts`.

- [ ] **Step 3: Implement `health_counts` in `board.rs`**

Add inside `impl BoardRow` (e.g. after `count`):

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p paavo-db --test board_ops health_counts`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-db/src/board.rs crates/paavo-db/tests/board_ops.rs
git commit -m "feat(db): BoardRow::health_counts — total + quarantined tally"
```

---

## Task 6: `paavo-db` — `BoardRow::list_dashboard`

**Files:**
- Modify: `crates/paavo-db/src/board.rs`
- Test: `crates/paavo-db/tests/board_ops.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/paavo-db/tests/board_ops.rs`:

```rust
#[test]
fn list_dashboard_orders_quarantined_first_then_recently_used() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();

    // Healthy, used most recently.
    let mut a = sample_board();
    a.id = "board-a".into();
    BoardRow::insert(db.raw_conn(), &a, now).unwrap();
    BoardRow::touch_last_used(db.raw_conn(), "board-a", now + 2000).unwrap();

    // Healthy, used a while ago.
    let mut b = sample_board();
    b.id = "board-b".into();
    BoardRow::insert(db.raw_conn(), &b, now).unwrap();
    BoardRow::touch_last_used(db.raw_conn(), "board-b", now + 1000).unwrap();

    // Healthy, never used (last_used_at NULL).
    let mut c = sample_board();
    c.id = "board-c".into();
    BoardRow::insert(db.raw_conn(), &c, now).unwrap();

    // Quarantined (never used) — must lead regardless of last_used_at.
    let mut d = sample_board();
    d.id = "board-d".into();
    BoardRow::insert(db.raw_conn(), &d, now).unwrap();
    BoardRow::quarantine(db.raw_conn(), "board-d", "broken").unwrap();

    let rows = BoardRow::list_dashboard(db.raw_conn(), 8).unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.spec.id.as_str()).collect();
    assert_eq!(ids, vec!["board-d", "board-a", "board-b", "board-c"]);

    // Limit is honoured.
    let top2 = BoardRow::list_dashboard(db.raw_conn(), 2).unwrap();
    assert_eq!(top2.len(), 2);
    assert_eq!(top2[0].spec.id, "board-d");
    assert_eq!(top2[1].spec.id, "board-a");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p paavo-db --test board_ops list_dashboard`
Expected: FAIL to compile — `no function or associated item named list_dashboard`.

- [ ] **Step 3: Implement `list_dashboard` in `board.rs`**

Add inside `impl BoardRow` (e.g. after `list_page`):

```rust
    /// The N most operationally-relevant boards for the dashboard fleet
    /// card: quarantined first, then most-recently-used, "never used"
    /// last (`last_used_at` NULL sorts last under DESC), `id` as the
    /// deterministic tiebreak.
    pub fn list_dashboard(conn: &Connection, limit: u32) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT * FROM board \
             ORDER BY (health = 'quarantined') DESC, last_used_at DESC, id ASC \
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p paavo-db --test board_ops list_dashboard`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-db/src/board.rs crates/paavo-db/tests/board_ops.rs
git commit -m "feat(db): BoardRow::list_dashboard — quarantined-first, LRU slice"
```

---

## Task 7: `paavo-web` — RO façade wrappers

**Files:**
- Modify: `crates/paavo-web/src/db.rs`

- [ ] **Step 1: Add three wrappers to `WebDb`**

Add inside `impl WebDb` in `crates/paavo-web/src/db.rs` (e.g. after `boards_count`):

```rust
    /// All-time job counts by state (SQL GROUP BY aggregate). Backs the
    /// dashboard stat cards.
    pub fn job_state_counts(&self) -> paavo_db::Result<paavo_proto::JobStateCounts> {
        paavo_db::JobRow::state_counts(self.inner.lock().raw_conn())
    }

    /// Board fleet health tally (total + quarantined). Backs the
    /// dashboard "Boards" stat card.
    pub fn board_health_counts(&self) -> paavo_db::Result<paavo_proto::BoardHealthCounts> {
        paavo_db::BoardRow::health_counts(self.inner.lock().raw_conn())
    }

    /// The dashboard fleet slice: quarantined-first then most-recently-used,
    /// capped at `limit` (see [`paavo_db::BoardRow::list_dashboard`]).
    pub fn boards_dashboard(&self, limit: u32) -> paavo_db::Result<Vec<paavo_db::BoardRow>> {
        paavo_db::BoardRow::list_dashboard(self.inner.lock().raw_conn(), limit)
    }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p paavo-web`
Expected: builds clean (no warnings).

- [ ] **Step 3: Commit**

```bash
git add crates/paavo-web/src/db.rs
git commit -m "feat(web): WebDb façade for dashboard aggregate queries"
```

---

## Task 8: `paavo-web` — `GET /api/dashboard` handler

**Files:**
- Create: `crates/paavo-web/src/api/dashboard.rs`
- Modify: `crates/paavo-web/src/api/boards.rs` (make `board_view` `pub(crate)`)
- Modify: `crates/paavo-web/src/api/mod.rs`
- Modify: `crates/paavo-web/src/app.rs`
- Test: `crates/paavo-web/tests/api_dashboard.rs`

- [ ] **Step 1: Write the failing integration test**

Create `crates/paavo-web/tests/api_dashboard.rs`:

```rust
//! Integration test for `GET /api/dashboard`: SQL aggregate counts +
//! the recent-jobs (in-memory index) and fleet (SQL) display slices.
//!
//! Counts/fleet read sqlite directly (immediate via WAL); recent_jobs is
//! poller-maintained, so the test polls the endpoint until the index
//! reflects the seeded jobs before asserting.

use axum::body::{to_bytes, Body};
use axum::http::Request;
use paavo_db::{BoardRow, Db, JobRow, NewJob};
use paavo_proto::{
    BoardHealth, BoardSelector, BoardSpec, DashboardOverview, JobId, JobSource, Priority,
    ProbeSelector,
};
use paavo_web::db::WebDb;
use paavo_web::index::LiveState;
use paavo_web::proxy::{AppState, PaavodClient};
use std::time::Duration;
use tempfile::tempdir;
use tower::ServiceExt;

fn app(interval: Duration) -> (tempfile::TempDir, Db, axum::Router) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let rw = Db::open(&path).unwrap();
    let webdb = WebDb::open(&path).unwrap();
    let live = LiveState::new();
    paavo_web::index::spawn_poller(webdb.clone(), live.clone(), interval);
    let paavod = PaavodClient::new("http://127.0.0.1:1").expect("valid URL");
    let state = AppState { db: webdb, paavod, live };
    let app = paavo_web::app::build_router(state);
    (dir, rw, app)
}

fn board(id: &str) -> BoardSpec {
    BoardSpec {
        id: id.into(),
        kind: "mcxa266".into(),
        probe_selector: ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: "ABC".into(),
        },
        chip_name: "MCXA266VFL".into(),
        target_name: "frdm-mcx-a266".into(),
        wiring_profile: Some("default".into()),
        health: BoardHealth::Healthy,
    }
}

fn new_job(id: JobId, submitter: &str) -> NewJob {
    NewJob {
        id,
        priority: Priority::Interactive,
        submitter: submitter.into(),
        source: JobSource::Cli,
        board_selector: BoardSelector {
            kind: "mcxa266".into(),
            instance: None,
            wiring_profile: None,
        },
        inactivity_timeout_ms: 120_000,
        hard_max_ms: 900_000,
        tar_blake3: "deadbeef".into(),
        tar_path: "/tmp/x.tar".into(),
        cargo_update_packages: vec![],
        skip_cache: false,
    }
}

async fn get_overview(app: &axum::Router) -> DashboardOverview {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "GET /api/dashboard not 200");
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("DashboardOverview JSON")
}

/// Poll until the in-memory index carries `want` recent jobs.
async fn wait_for_recent(app: &axum::Router, want: usize) -> DashboardOverview {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let ov = get_overview(app).await;
        if ov.recent_jobs.len() == want {
            return ov;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "index never reached {want} recent jobs (last {})",
            ov.recent_jobs.len()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn dashboard_reports_sql_counts_recent_jobs_and_fleet() {
    let (_dir, rw, app) = app(Duration::from_millis(20));

    // Two boards: one healthy, one quarantined.
    BoardRow::insert(rw.raw_conn(), &board("z-healthy"), 0).unwrap();
    BoardRow::insert(rw.raw_conn(), &board("a-quarantined"), 0).unwrap();
    BoardRow::quarantine(rw.raw_conn(), "a-quarantined", "broken").unwrap();

    // Two submitted jobs; bob is newer than alice.
    JobRow::insert(rw.raw_conn(), &new_job(JobId::new(), "alice"), 1000).unwrap();
    JobRow::insert(rw.raw_conn(), &new_job(JobId::new(), "bob"), 2000).unwrap();

    let ov = wait_for_recent(&app, 2).await;

    // Job state counts (SQL, exact).
    assert_eq!(ov.jobs.submitted, 2);
    assert_eq!(ov.jobs.queue(), 2);
    assert_eq!(ov.jobs.terminal(), 0);
    assert_eq!(ov.jobs.pass_rate_pct(), None);

    // Board health counts (SQL, exact).
    assert_eq!(ov.boards.total, 2);
    assert_eq!(ov.boards.quarantined, 1);
    assert_eq!(ov.boards.healthy(), 1);

    // Recent jobs: newest-first.
    assert_eq!(ov.recent_jobs.len(), 2);
    assert_eq!(ov.recent_jobs[0].submitter, "bob");
    assert_eq!(ov.recent_jobs[1].submitter, "alice");

    // Fleet slice: quarantined board leads.
    assert!(!ov.fleet.is_empty());
    assert_eq!(ov.fleet[0].spec.id, "a-quarantined");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p paavo-web --test api_dashboard`
Expected: the test **compiles** (every referenced item already exists) but **fails at runtime**. With no `/api/dashboard` route, axum's asset fallback serves the SPA HTML shell at `200`, so the `assert_eq!(status, 200)` passes but `serde_json::from_slice::<DashboardOverview>` then panics (`DashboardOverview JSON`) trying to parse HTML. Proceed to implement the route.

- [ ] **Step 3: Make `board_view` reusable in `boards.rs`**

In `crates/paavo-web/src/api/boards.rs`, change the function signature from:

```rust
fn board_view(r: paavo_db::BoardRow) -> paavo_proto::BoardView {
```

to:

```rust
pub(crate) fn board_view(r: paavo_db::BoardRow) -> paavo_proto::BoardView {
```

- [ ] **Step 4: Create the handler `api/dashboard.rs`**

Create `crates/paavo-web/src/api/dashboard.rs`:

```rust
//! `GET /api/dashboard` — the consolidated landing-page payload.
//!
//! One bounded response for the dashboard: exact SQL aggregate counts
//! (`job_state_counts`, `board_health_counts`) plus the two short display
//! lists the page renders — the 8 newest jobs (from the poller-maintained
//! in-memory index, so the jobs list never touches sqlite on the request
//! path) and the relevant fleet slice (`boards_dashboard`, SQL). Its size
//! does not grow with the fleet or job history.
use crate::api::boards::board_view;
use crate::proxy::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use paavo_proto::DashboardOverview;

/// Newest jobs shown in the "Recent activity" table.
const RECENT_JOBS: u32 = 8;
/// Boards shown in the "Board fleet" table (quarantined-first, LRU).
const FLEET_SLICE: u32 = 8;

/// `GET /api/dashboard` — see the module docs. Extracts the whole
/// `AppState`: it needs the DB (counts + fleet slice) and the live state
/// (recent-jobs index + current revisions). There is no `.await` between
/// taking the index read-guard and dropping it, so no lock is held across
/// a suspension point.
pub async fn get(
    State(s): State<AppState>,
) -> Result<Json<DashboardOverview>, (StatusCode, String)> {
    let err = |e: paavo_db::DbError| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let jobs = s.db.job_state_counts().map_err(err)?;
    let boards = s.db.board_health_counts().map_err(err)?;
    let fleet = s
        .db
        .boards_dashboard(FLEET_SLICE)
        .map_err(err)?
        .into_iter()
        .map(board_view)
        .collect();
    let recent_jobs = {
        let (items, _) = s.live.index.read().search("", None, 1, RECENT_JOBS);
        items
    };
    let rev = s.live.revisions();
    Ok(Json(DashboardOverview {
        jobs,
        boards,
        recent_jobs,
        fleet,
        jobs_revision: rev.jobs,
        boards_revision: rev.boards,
    }))
}
```

- [ ] **Step 5: Register the module + route**

In `crates/paavo-web/src/api/mod.rs`, add `pub mod dashboard;` (keep alphabetical):

```rust
pub mod boards;
pub mod dashboard;
pub mod events;
pub mod jobs;
pub mod schedules;
```

In `crates/paavo-web/src/app.rs`, add the route (after the `/api/boards` line):

```rust
        .route("/api/dashboard", get(crate::api::dashboard::get))
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p paavo-web --test api_dashboard`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/paavo-web/src/api/dashboard.rs crates/paavo-web/src/api/boards.rs crates/paavo-web/src/api/mod.rs crates/paavo-web/src/app.rs crates/paavo-web/tests/api_dashboard.rs
git commit -m "feat(web): GET /api/dashboard — consolidated overview endpoint"
```

---

## Task 9: `paavo-web-ui` — fetch wrapper + dashboard rewrite

**Files:**
- Modify: `crates/paavo-web-ui/src/api.rs`
- Modify: `crates/paavo-web-ui/src/components/dashboard.rs`

> `paavo-web-ui` is workspace-excluded (wasm32); it is not built by `cargo build/test --workspace`. Verify this task with `just build-ui` (Task 10, Step 3).

- [ ] **Step 1: Add the fetch wrapper to `api.rs`**

In `crates/paavo-web-ui/src/api.rs`, change the import line:

```rust
use paavo_proto::{BoardView, JobListItem, JobView, LogFrame, Page, ScheduleView};
```

to add `DashboardOverview`:

```rust
use paavo_proto::{
    BoardView, DashboardOverview, JobListItem, JobView, LogFrame, Page, ScheduleView,
};
```

Add this function (e.g. after `boards`):

```rust
/// `GET /api/dashboard` — the consolidated landing-page payload: exact
/// aggregate counts plus the recent-jobs and fleet display slices, in one
/// bounded response. Replaces the dashboard's old wide jobs+boards fetch.
pub async fn dashboard() -> Result<DashboardOverview, String> {
    fetch_json("/api/dashboard").await
}
```

- [ ] **Step 2: Rewrite `components/dashboard.rs`**

Replace the entire contents of `crates/paavo-web-ui/src/components/dashboard.rs` with:

```rust
//! Dashboard landing page (`/`).
//!
//! The at-a-glance operator view, derived from a single consolidated
//! fetch: [`api::dashboard`] returns exact SQL aggregate counts plus two
//! short display lists (the 8 newest jobs and the relevant fleet slice).
//! The [`LocalResource`] is keyed on both the `jobs` and `boards` live
//! revisions, so a server-pushed bump on either refetches and the whole
//! dashboard recomputes in place.
//!
//! ## Accuracy of the counts
//!
//! The stat cards are exact at any scale: the counts are computed by the
//! database (`COUNT(*) ... GROUP BY state`, board health tally), not by
//! counting a capped page in the browser. "Pass rate" is all-time over
//! every retained job (bounded by the retention window). The fleet list
//! is intentionally a small, relevant slice (quarantined first, then
//! most-recently-used); the "Boards" card still reports the true
//! healthy/total for the whole fleet.

use leptos::prelude::*;
use leptos_router::components::A;
use paavo_proto::DashboardOverview;

use crate::api;
use crate::components::widgets::{abs_time, rel_time, HealthBadge, StateBadge};
use crate::live::LiveSignals;

/// The `/` landing page.
#[component]
pub fn Dashboard() -> impl IntoView {
    let live = expect_context::<LiveSignals>();

    // One consolidated fetch, refetched when either the jobs or the
    // boards revision bumps.
    let over_res = LocalResource::new(move || {
        let _ = live.jobs.get();
        let _ = live.boards.get();
        async move { api::dashboard().await }
    });

    view! {
        <h1>"Dashboard"</h1>
        {move || {
            match over_res.get().map(|w| (*w).clone()) {
                Some(Ok(o)) => render(o).into_any(),
                Some(Err(e)) => {
                    view! { <p class="muted">{format!("failed to load dashboard: {e}")}</p> }
                        .into_any()
                }
                None => view! { <p class="muted">"loading…"</p> }.into_any(),
            }
        }}
    }
}

/// One stat card: a big value, its label, and an optional muted subtitle.
fn stat_card(value: String, label: &'static str, sub: Option<String>) -> impl IntoView {
    view! {
        <div class="stat card">
            <span class="stat-value">{value}</span>
            <span class="stat-label">{label}</span>
            {sub.map(|s| view! { <span class="stat-sub">{s}</span> })}
        </div>
    }
}

/// Build the full dashboard from the consolidated overview: the stat-card
/// grid on top, then a two-column row of recent activity (wider) + the
/// board fleet (narrower) that collapses to one column on narrow screens.
fn render(over: DashboardOverview) -> impl IntoView {
    // --- stat tallies (exact, from SQL aggregates) ---
    let running = over.jobs.running;
    let queue = over.jobs.queue();
    let terminal = over.jobs.terminal();
    let pass_rate = match over.jobs.pass_rate_pct() {
        Some(p) => format!("{p}%"),
        None => "—".to_string(),
    };
    let fleet_total = over.boards.total;
    let healthy = over.boards.healthy();
    let quarantined = over.boards.quarantined;

    // --- recent activity: the newest jobs (already capped server-side) ---
    let recent = over
        .recent_jobs
        .iter()
        .map(|it| {
            let id = it.id.to_string();
            let href = format!("/jobs/{id}");
            let submitted_rel = rel_time(it.submitted_at);
            let submitted_abs = abs_time(it.submitted_at);
            view! {
                <tr>
                    <td>
                        <A href=href>
                            <span class="mono">{id}</span>
                        </A>
                    </td>
                    <td>
                        <StateBadge state=it.state />
                    </td>
                    <td>{it.submitter.clone()}</td>
                    <td>
                        <span title=submitted_abs>{submitted_rel}</span>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>();
    let no_recent = recent.is_empty();

    // --- board fleet: the relevant slice (quarantined first, then LRU) ---
    let fleet = over
        .fleet
        .iter()
        .map(|b| {
            let id = b.spec.id.clone();
            let (lu_rel, lu_abs) = match b.last_used_at {
                Some(t) => (rel_time(t), abs_time(t)),
                None => ("never".to_string(), String::new()),
            };
            view! {
                <tr>
                    <td>
                        <span class="mono">{id}</span>
                    </td>
                    <td>
                        <HealthBadge health=b.spec.health />
                    </td>
                    <td>
                        <span title=lu_abs>{lu_rel}</span>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>();
    let no_fleet = fleet.is_empty();

    view! {
        <div class="stats">
            {stat_card(running.to_string(), "Running", None)}
            {stat_card(queue.to_string(), "Queue", None)}
            {stat_card(
                format!("{healthy}/{fleet_total}"),
                "Boards",
                (quarantined > 0).then(|| format!("{quarantined} quarantined")),
            )}
            {stat_card(pass_rate, "Pass rate", Some(format!("{terminal} runs")))}
        </div>
        <div class="grid2">
            <div class="card">
                <h2 class="card-title">"Recent activity"</h2>
                <table class="table">
                    <tbody>
                        {recent}
                        {no_recent
                            .then(|| {
                                view! {
                                    <tr>
                                        <td colspan="4" class="muted">"no jobs yet"</td>
                                    </tr>
                                }
                            })}
                    </tbody>
                </table>
            </div>
            <div class="card">
                <h2 class="card-title">"Board fleet"</h2>
                <table class="table">
                    <tbody>
                        {fleet}
                        {no_fleet
                            .then(|| {
                                view! {
                                    <tr>
                                        <td colspan="3" class="muted">"no boards registered"</td>
                                    </tr>
                                }
                            })}
                    </tbody>
                </table>
            </div>
        </div>
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/paavo-web-ui/src/api.rs crates/paavo-web-ui/src/components/dashboard.rs
git commit -m "feat(web-ui): dashboard reads SQL aggregates via /api/dashboard"
```

---

## Task 10: Full verification

**Files:** none (verification only).

- [ ] **Step 1: Format check**

Run: `cargo fmt --all -- --check`
Expected: no output (clean). If it reports diffs, run `cargo fmt --all` and re-commit the touched files.

- [ ] **Step 2: Clippy + tests (what CI runs)**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings, no errors.

Run: `cargo test --workspace`
Expected: all green, including the new `paavo-proto` `stats` tests, the `paavo-db` `state_counts` / `health_counts` / `list_dashboard` tests, and `paavo-web` `api_dashboard`.

- [ ] **Step 3: Build the WASM UI**

Run: `just build-ui`
Expected: `trunk build --release` completes; `crates/paavo-web-ui/dist` is produced with no compile errors in the rewritten `dashboard.rs`.

- [ ] **Step 4: Manual smoke (optional but recommended)**

In one shell: `PAAVO_FAKE_RUNNER=1 cargo run -p paavod -- --config sample-paavo.toml`
In another: `cargo run -p paavo-web -- --config sample-paavo.toml`, open `/`.
Submit jobs (`PAAVO_HOST=http://127.0.0.1:8090 cargo run -p paavo-cli -- run ...`) and confirm the "Running" / "Queue" / "Pass rate" cards and "Recent activity" update live, and that the "Boards" card shows the correct healthy/total.

- [ ] **Step 5: Update `AGENTS.md` if needed**

The dashboard endpoint set changed (new `/api/dashboard`). If the route list or data-flow description in `AGENTS.md` references the dashboard's data sourcing, update it. Commit any change:

```bash
git add AGENTS.md
git commit -m "docs: note /api/dashboard overview endpoint"
```

---

## Self-Review Notes

- **Spec coverage:** §4.1 → Tasks 1–3; §4.2 → Tasks 4–6; §4.3 → Tasks 7–8; §4.4 → Task 9; §6 testing → tests embedded per task + Task 10; §3.2 (all-time pass rate) → `JobStateCounts::pass_rate_pct` + label "Pass rate" (no "(recent)"); §3.4 (fleet slice) → Task 6 + Task 9; §3.5 (counts SQL / recent from index) → Task 8 handler.
- **Type consistency:** `JobStateCounts`, `BoardHealthCounts`, `DashboardOverview` field/method names (`queue`, `terminal`, `pass_rate_pct`, `healthy`, `recent_jobs`, `fleet`, `jobs_revision`, `boards_revision`) are identical across proto, the web handler, and the UI. DB methods (`state_counts`, `health_counts`, `list_dashboard`) match their façade wrappers (`job_state_counts`, `board_health_counts`, `boards_dashboard`) and handler call sites.
- **No migration:** `idx_job_state` already exists (`V1`/`V4`); no schema change is introduced.
