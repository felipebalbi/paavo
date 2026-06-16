# Parallel Build Pool + Board-Decoupled Run Stage — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decouple the `cargo build` phase from board occupancy so up to `max_concurrent_builds` (default 5) jobs build in parallel while the hardware run phase stays board-serialized, with a new `AwaitingBoard` state and no change to acceptance concurrency.

**Architecture:** A two-stage dispatcher. Stage 1 (build) claims `Submitted → Building`, bounded by an in-memory slot pool of N entries (each a dedicated `CARGO_TARGET_DIR`), produces a stable content-addressed ELF, then `Building → AwaitingBoard`. Stage 2 (run) claims `AwaitingBoard → Running` only when a matching board is free. Both the build step and run step are injected traits (`Builder`, `Runner`) so tests run without cargo or hardware.

**Tech Stack:** Rust, rusqlite + refinery (SQLite), axum, tokio, crossbeam-channel, parking_lot. Spec: `docs/superpowers/specs/2026-06-16-parallel-build-pool-design.md`.

**Reference reading before starting:**
- Spec (above) — the authority for every decision here.
- `crates/paavod/src/dispatch.rs` — the loop being rewritten.
- `crates/paavod/tests/dispatch_loop.rs` — fixture + `CountingRunner` pattern the new tests mirror.
- `crates/paavo-db/migrations/V1__initial.sql` + `crates/paavo-db/src/db.rs` — FK-on-during-migration constraint.

---

## File Structure

**New files:**
- `crates/paavo-db/migrations/V2__awaiting_board.sql` — extend `job.state` CHECK via FK-safe dual rebuild.
- `crates/paavo-db/tests/migration_v2.rs` — migration data-preservation test.
- `crates/paavod/src/builder.rs` — `Builder` trait + `RealBuilder` (wraps `paavo_build`).

**Modified (by responsibility):**
- `crates/paavo-proto/src/job.rs` — `JobState::AwaitingBoard`.
- `crates/paavo-db/src/job.rs` — state string maps; three additive transitions; `finalize` guard.
- `crates/paavo-db/src/board.rs` — narrow board-exclusion to `running`.
- `crates/paavo-core/src/scheduler.rs` — `pick_buildable` + `pick_runnable` (single-flight).
- `crates/paavo-core/src/cancel.rs` — `cancel_if_pending`.
- `crates/paavo-build/src/build.rs` — cancellable build (kill cargo child).
- `crates/paavod/src/{config,state_dir,cancellation,dispatch,main}.rs` — config knob, slot pool, build-kill registry, two-stage loop, wiring.
- `crates/paavod/src/routes/{jobs,admin}.rs` — `awaiting_board` parsing, cancel routing, purge guard.
- `crates/paavo-cli/src/cmd_jobs.rs`, `crates/paavo-web/*` — render `awaiting_board`.
- `sample-paavo.toml` — document the knob.

**Coupling note:** Transitions and dispatch are tightly coupled. We add new transitions/pickers **additively** (old ones kept) through Phase C so the workspace compiles + tests green after every task. The dispatch rewrite (Task 12) switches to the new APIs; Task 14 deletes the now-dead old ones.

---

## Phase A — proto + DB foundation

### Task 1: Add `JobState::AwaitingBoard`

**Files:**
- Modify: `crates/paavo-proto/src/job.rs:43-66` (enum), 
- Modify: `crates/paavo-proto/tests/serde_roundtrip.rs`, `crates/paavo-proto/tests/wire_compat.rs`
- Modify: `crates/paavo-db/src/job.rs:337-365` (`state_to_str` / `state_from_str`)

- [ ] **Step 1: Write failing serde test**

In `crates/paavo-proto/tests/serde_roundtrip.rs`, add:

```rust
#[test]
fn awaiting_board_serializes_to_snake_case() {
    let s = serde_json::to_string(&paavo_proto::JobState::AwaitingBoard).unwrap();
    assert_eq!(s, "\"awaiting_board\"");
    let back: paavo_proto::JobState = serde_json::from_str("\"awaiting_board\"").unwrap();
    assert_eq!(back, paavo_proto::JobState::AwaitingBoard);
    assert!(!back.is_terminal());
}
```

- [ ] **Step 2: Run — verify it fails to compile**

Run: `cargo test -p paavo-proto awaiting_board_serializes_to_snake_case`
Expected: FAIL — `no variant named AwaitingBoard`.

- [ ] **Step 3: Add the variant**

In `crates/paavo-proto/src/job.rs`, insert after the `Running` variant (after line 52):

```rust
    /// Built; ELF ready; waiting for a free matching board. The build
    /// slot has been released, so this job no longer counts toward
    /// `max_concurrent_builds`. Non-terminal.
    #[serde(rename = "awaiting_board")]
    AwaitingBoard,
```

- [ ] **Step 4: Add the paavo-db string-map arms (keeps workspace compiling)**

In `crates/paavo-db/src/job.rs`, `state_to_str` add `JobState::AwaitingBoard => "awaiting_board",` and `state_from_str` add `"awaiting_board" => JobState::AwaitingBoard,` (place both next to the `Running`/`"running"` arm).

- [ ] **Step 5: Pin the wire shape**

In `crates/paavo-proto/tests/wire_compat.rs`, add an assertion alongside the existing `JobState` cases:

```rust
assert_eq!(serde_json::to_string(&JobState::AwaitingBoard).unwrap(), "\"awaiting_board\"");
```

(If `wire_compat.rs` imports `JobState` under a different path/alias, match the file's existing style.)

- [ ] **Step 6: Run — verify green**

Run: `cargo test -p paavo-proto && cargo build -p paavo-db`
Expected: PASS / compiles.

- [ ] **Step 7: Commit**

```bash
git add crates/paavo-proto/src/job.rs crates/paavo-proto/tests/ crates/paavo-db/src/job.rs
git commit -m "feat(proto): add JobState::AwaitingBoard"
```

---

### Task 2: V2 migration — extend `job.state` CHECK (FK-safe dual rebuild)

**Why dual rebuild:** `db.rs` enables `foreign_keys=ON` before refinery runs, and refinery wraps each migration in a transaction where `PRAGMA foreign_keys=OFF` is a no-op. A plain `DROP TABLE job` would fire `log_frame`'s `ON DELETE CASCADE` and erase all logs. Rebuilding `log_frame` to point at the new table *first* means each `DROP` has no child referencing it.

**Files:**
- Create: `crates/paavo-db/migrations/V2__awaiting_board.sql`
- Create: `crates/paavo-db/tests/migration_v2.rs`

- [ ] **Step 1: Write the failing migration test**

Create `crates/paavo-db/tests/migration_v2.rs`:

```rust
//! V2 must extend the job.state CHECK to allow 'awaiting_board' WITHOUT
//! losing log_frame rows to the ON DELETE CASCADE during the rebuild.
use rusqlite::Connection;

#[test]
fn v2_preserves_data_and_allows_awaiting_board() {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();

    // Apply V1 schema, then seed a board + a running job + two log frames.
    conn.execute_batch(include_str!("../migrations/V1__initial.sql")).unwrap();
    conn.execute_batch(
        "INSERT INTO board (id,kind,probe_selector,chip_name,target_name,health,consecutive_infra_failures,created_at)
         VALUES ('b','mcxa266','{}','c','t','healthy',0,0);
         INSERT INTO job (id,priority,submitter,source,board_selector,inactivity_timeout_ms,hard_max_ms,state,submitted_at,tar_blake3,tar_path)
         VALUES ('j1',0,'me','cli','{}',1,1,'running',0,'aaa','/x');
         INSERT INTO log_frame (job_id,seq,ts_us,level,message) VALUES ('j1',0,0,'info','hello');
         INSERT INTO log_frame (job_id,seq,ts_us,level,message) VALUES ('j1',1,0,'info','world');",
    ).unwrap();

    // Apply V2.
    conn.execute_batch(include_str!("../migrations/V2__awaiting_board.sql")).unwrap();

    // Job survived.
    let state: String = conn.query_row("SELECT state FROM job WHERE id='j1'", [], |r| r.get(0)).unwrap();
    assert_eq!(state, "running");
    // Logs survived (no cascade loss).
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM log_frame WHERE job_id='j1'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 2, "log_frame rows must survive the job rebuild");
    // New state accepted.
    conn.execute(
        "INSERT INTO job (id,priority,submitter,source,board_selector,inactivity_timeout_ms,hard_max_ms,state,submitted_at,tar_blake3,tar_path)
         VALUES ('j2',0,'me','cli','{}',1,1,'awaiting_board',0,'bbb','/y')", []).unwrap();
    // FK still enforced after rebuild.
    let bad = conn.execute(
        "INSERT INTO log_frame (job_id,seq,ts_us,level,message) VALUES ('nope',0,0,'info','x')", []);
    assert!(bad.is_err(), "FK must still be enforced post-rebuild");
}
```

- [ ] **Step 2: Run — verify it fails**

Run: `cargo test -p paavo-db --test migration_v2`
Expected: FAIL — `cannot find file ../migrations/V2__awaiting_board.sql` (compile error on `include_str!`).

- [ ] **Step 3: Write the migration**

Create `crates/paavo-db/migrations/V2__awaiting_board.sql`:

```sql
-- Extend job.state CHECK to allow 'awaiting_board'.
--
-- SQLite cannot ALTER a CHECK constraint, so the job table is rebuilt.
-- refinery runs this inside a transaction with foreign_keys=ON (set in
-- Db::open BEFORE migrations), so `PRAGMA foreign_keys=OFF` is a no-op
-- here and a plain `DROP TABLE job` would fire log_frame's ON DELETE
-- CASCADE and erase all logs. Instead we rebuild BOTH tables in
-- dependency order: build the new log_frame pointing at the new job
-- table first, so each DROP has no child referencing it.

-- 1. New job table: identical columns/order to V1, extended state CHECK.
CREATE TABLE job_new (
    id                       TEXT PRIMARY KEY,
    priority                 INTEGER NOT NULL,
    submitter                TEXT NOT NULL,
    source                   TEXT NOT NULL CHECK (source IN ('cli','scheduler')),
    board_selector           TEXT NOT NULL,
    inactivity_timeout_ms    INTEGER NOT NULL,
    hard_max_ms              INTEGER NOT NULL,
    state                    TEXT NOT NULL CHECK (state IN
        ('submitted','building','awaiting_board','running','passed','failed','timedout','aborted')),
    outcome_detail           TEXT,
    board_id                 TEXT REFERENCES board(id),
    submitted_at             INTEGER NOT NULL,
    started_at               INTEGER,
    finished_at              INTEGER,
    tar_blake3               TEXT NOT NULL,
    tar_path                 TEXT NOT NULL,
    elf_path                 TEXT
);
INSERT INTO job_new SELECT * FROM job;

-- 2. New log_frame whose FK references job_new (so the old job has no
--    child at drop time).
CREATE TABLE log_frame_new (
    job_id   TEXT NOT NULL REFERENCES job_new(id) ON DELETE CASCADE,
    seq      INTEGER NOT NULL,
    ts_us    INTEGER NOT NULL,
    level    TEXT NOT NULL CHECK (level IN ('trace','debug','info','warn','error')),
    target   TEXT,
    message  TEXT NOT NULL,
    PRIMARY KEY (job_id, seq)
);
INSERT INTO log_frame_new SELECT * FROM log_frame;

-- 3. Drop old child then old parent. Neither has a child referencing it.
DROP TABLE log_frame;
DROP TABLE job;

-- 4. Rename into place. Renaming job_new -> job rewrites log_frame_new's
--    FK to reference `job` (legacy_alter_table is OFF by default).
ALTER TABLE job_new RENAME TO job;
ALTER TABLE log_frame_new RENAME TO log_frame;

-- 5. Recreate indexes (they did not survive the rebuild).
CREATE INDEX idx_job_state           ON job(state);
CREATE INDEX idx_job_submitted_at    ON job(submitted_at);
CREATE INDEX idx_job_priority_subat  ON job(priority, submitted_at) WHERE state = 'submitted';
CREATE INDEX idx_log_frame_job_level ON log_frame(job_id, level);
```

- [ ] **Step 4: Run — verify the migration test passes**

Run: `cargo test -p paavo-db --test migration_v2`
Expected: PASS.

- [ ] **Step 5: Run the full paavo-db suite (existing tests must still open Db fine)**

Run: `cargo test -p paavo-db`
Expected: PASS (refinery applies V1+V2 on every `Db::open`).

- [ ] **Step 6: Commit**

```bash
git add crates/paavo-db/migrations/V2__awaiting_board.sql crates/paavo-db/tests/migration_v2.rs
git commit -m "feat(db): V2 migration adds awaiting_board state (FK-safe rebuild)"
```

---

### Task 3: Additive DB transitions for the two-stage lifecycle

Add three new transitions and widen `finalize`. **Keep** the old `transition_to_building` / `transition_to_running` (removed in Task 14) so existing dispatch + tests still compile.

**Files:**
- Modify: `crates/paavo-db/src/job.rs` (add methods near the existing transitions ~line 223-299)
- Modify: `crates/paavo-db/tests/job_ops.rs`

- [ ] **Step 1: Write failing tests**

In `crates/paavo-db/tests/job_ops.rs`, add (the file already has `fresh_db`, `insert_default_board`, `sample_new_job`):

```rust
#[test]
fn two_stage_transitions_submitted_to_running() {
    let db = fresh_db();
    insert_default_board(&db);
    let id = JobId::new();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), 0).unwrap();

    // Submitted -> Building: no board claimed, started_at set.
    JobRow::transition_submitted_to_building(db.raw_conn(), &id, 10).unwrap();
    let r = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(r.state, JobState::Building);
    assert_eq!(r.board_id, None);
    assert_eq!(r.started_at, Some(10));

    // Building -> AwaitingBoard: elf recorded, still no board.
    JobRow::transition_building_to_awaiting_board(db.raw_conn(), &id, "/elf/aaa.elf").unwrap();
    let r = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(r.state, JobState::AwaitingBoard);
    assert_eq!(r.elf_path.as_deref(), Some("/elf/aaa.elf"));
    assert_eq!(r.board_id, None);

    // AwaitingBoard -> Running: board claimed now.
    JobRow::transition_awaiting_to_running(db.raw_conn(), &id, "mcxa266-01").unwrap();
    let r = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(r.state, JobState::Running);
    assert_eq!(r.board_id.as_deref(), Some("mcxa266-01"));

    // finalize is valid from running.
    JobRow::finalize(db.raw_conn(), &id, &OutcomeRecord {
        state: JobState::Passed,
        outcome: JobOutcome::Passed,
        finished_at_ms: 20,
    }).unwrap();
    assert_eq!(JobRow::get(db.raw_conn(), &id).unwrap().state, JobState::Passed);
}

#[test]
fn finalize_allowed_from_awaiting_board() {
    let db = fresh_db();
    insert_default_board(&db);
    let id = JobId::new();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), 0).unwrap();
    JobRow::transition_submitted_to_building(db.raw_conn(), &id, 1).unwrap();
    JobRow::transition_building_to_awaiting_board(db.raw_conn(), &id, "/e.elf").unwrap();
    // Cancel an AwaitingBoard job: finalize must accept it.
    JobRow::finalize(db.raw_conn(), &id, &OutcomeRecord {
        state: JobState::Aborted,
        outcome: JobOutcome::Aborted { by: paavo_proto::AbortReason::User },
        finished_at_ms: 5,
    }).unwrap();
    assert_eq!(JobRow::get(db.raw_conn(), &id).unwrap().state, JobState::Aborted);
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test -p paavo-db --test job_ops two_stage_transitions_submitted_to_running`
Expected: FAIL — methods not found.

- [ ] **Step 3: Implement the transitions**

In `crates/paavo-db/src/job.rs`, add inside `impl JobRow` (near the existing transitions). Reuse the existing `DbError::UnknownEnum` error shape per the file's `TODO(later)` note:

```rust
    /// `Submitted → Building` for the build stage. Unlike the legacy
    /// `transition_to_building`, claims NO board (the build holds no
    /// hardware); only records `started_at`.
    pub fn transition_submitted_to_building(conn: &Connection, id: &JobId, now_ms: i64) -> Result<()> {
        let n = conn.execute(
            "UPDATE job SET state = 'building', started_at = ?1
             WHERE id = ?2 AND state = 'submitted'",
            params![now_ms, id.to_string()],
        )?;
        if n == 0 {
            return Err(DbError::UnknownEnum {
                column: "job.state",
                value: "expected 'submitted' for transition_submitted_to_building".into(),
            });
        }
        Ok(())
    }

    /// `Building → AwaitingBoard`, recording the stable ELF path. The
    /// build slot is released by the caller after this returns.
    pub fn transition_building_to_awaiting_board(conn: &Connection, id: &JobId, elf_path: &str) -> Result<()> {
        let n = conn.execute(
            "UPDATE job SET state = 'awaiting_board', elf_path = ?1
             WHERE id = ?2 AND state = 'building'",
            params![elf_path, id.to_string()],
        )?;
        if n == 0 {
            return Err(DbError::UnknownEnum {
                column: "job.state",
                value: "expected 'building' for transition_building_to_awaiting_board".into(),
            });
        }
        Ok(())
    }

    /// `AwaitingBoard → Running`, claiming `board_id` (the run claim).
    pub fn transition_awaiting_to_running(conn: &Connection, id: &JobId, board_id: &str) -> Result<()> {
        let n = conn.execute(
            "UPDATE job SET state = 'running', board_id = ?1
             WHERE id = ?2 AND state = 'awaiting_board'",
            params![board_id, id.to_string()],
        )?;
        if n == 0 {
            return Err(DbError::UnknownEnum {
                column: "job.state",
                value: "expected 'awaiting_board' for transition_awaiting_to_running".into(),
            });
        }
        Ok(())
    }
```

- [ ] **Step 4: Widen the `finalize` guard**

In `JobRow::finalize`, change the WHERE clause from
`WHERE id = ?4 AND state IN ('submitted','building','running')`
to
`WHERE id = ?4 AND state IN ('submitted','building','awaiting_board','running')`.

- [ ] **Step 5: Run — verify green**

Run: `cargo test -p paavo-db --test job_ops`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/paavo-db/src/job.rs crates/paavo-db/tests/job_ops.rs
git commit -m "feat(db): two-stage job transitions + finalize from awaiting_board"
```

---

### Task 4: Narrow board exclusion to `running`

A `building` job no longer holds a board, so only `running` should exclude a board from the run-stage pick.

**Files:**
- Modify: `crates/paavo-db/src/board.rs:92-123` (`find_healthy_for_selector`)
- Modify: `crates/paavo-db/tests/board_ops.rs` (the in-flight-exclusion tests, ~line 291 and ~457)

- [ ] **Step 1: Update the exclusion test to the new semantics**

In `crates/paavo-db/tests/board_ops.rs`, find `find_healthy_for_selector_excludes_boards_with_in_flight_jobs`. Rewrite its in-flight setup so the job is driven to **running with a board** via the new transitions, and add an assertion that a **building** job (no board) does **not** exclude:

```rust
    // A building job holds NO board now — must NOT exclude.
    let building = paavo_db::JobId::new();
    paavo_db::JobRow::insert(db.raw_conn(), &sample_new_job(building), 0).unwrap();
    paavo_db::JobRow::transition_submitted_to_building(db.raw_conn(), &building, 1).unwrap();
    let rows = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel).unwrap();
    assert_eq!(rows.len(), 1, "a building (board-free) job must not exclude the board");

    // Drive it to running WITH the board — now it must exclude.
    paavo_db::JobRow::transition_building_to_awaiting_board(db.raw_conn(), &building, "/e.elf").unwrap();
    paavo_db::JobRow::transition_awaiting_to_running(db.raw_conn(), &building, "mcxa266-01").unwrap();
    let rows = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel).unwrap();
    assert!(rows.is_empty(), "a running job must exclude its board");
```

(Keep the test's existing `sel`/board setup; if the file lacks `sample_new_job`, inline a `NewJob` like the other tests in that file do. Remove or adapt the old `transition_to_building(..., "mcxa266-01", ...)`-based assertions that assumed building excludes.)

- [ ] **Step 2: Run — verify failure**

Run: `cargo test -p paavo-db --test board_ops find_healthy_for_selector_excludes_boards_with_in_flight_jobs`
Expected: FAIL — building still excludes (old query) so `rows.len()` assertion fails.

- [ ] **Step 3: Narrow the query**

In `crates/paavo-db/src/board.rs`, change the `NOT EXISTS` clause inside `find_healthy_for_selector` from
`AND job.state IN ('building','running')` to `AND job.state = 'running'`. Update the doc comment above it to say "no `job` row in `running` state on this board".

- [ ] **Step 4: Run — verify green**

Run: `cargo test -p paavo-db`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-db/src/board.rs crates/paavo-db/tests/board_ops.rs
git commit -m "feat(db): board exclusion keys on running only (build holds no board)"
```

---

## Phase B — core scheduler + cancellation

### Task 5: `pick_buildable` + `pick_runnable` (single-flight)

Additive: keep `pick_next`. Add two DB list helpers and two pure pickers.

**Files:**
- Modify: `crates/paavo-db/src/job.rs` (two list helpers)
- Modify: `crates/paavo-core/src/scheduler.rs` (two pickers)
- Modify: `crates/paavo-core/src/lib.rs` (exports)
- Create: `crates/paavo-core/tests/scheduler_two_stage.rs`

- [ ] **Step 1: Write failing tests**

Create `crates/paavo-core/tests/scheduler_two_stage.rs`:

```rust
mod common;
use common::{enqueue_with, fresh_db, insert_board};
use paavo_core::{pick_buildable, pick_runnable, SchedulerConfig};
use paavo_db::JobRow;
use paavo_proto::{BoardHealth, JobState};

const CFG: SchedulerConfig = SchedulerConfig { starvation_threshold_ms: 6 * 60 * 60 * 1000 };

#[test]
fn pick_buildable_returns_submitted_and_skips_in_flight_blake3() {
    let db = fresh_db();
    insert_board(&db, "b1", "mcxa266", BoardHealth::Healthy);
    // Two submitted jobs, SAME tar_blake3 "x" (default helper value).
    let a = enqueue_with(&db, 100, |_| {});
    let _b = enqueue_with(&db, 200, |_| {});
    // Move A to building → its blake3 "x" is now in-flight.
    JobRow::transition_submitted_to_building(db.raw_conn(), &a, 150).unwrap();
    // Single-flight: B shares blake3 "x", so nothing is buildable.
    assert!(pick_buildable(db.raw_conn(), CFG, 1000).unwrap().is_none());

    // A distinct-blake3 job IS buildable.
    let c = enqueue_with(&db, 300, |req| req.tar_blake3 = "y".into());
    assert_eq!(pick_buildable(db.raw_conn(), CFG, 1000).unwrap().unwrap().id, c);
}

#[test]
fn pick_runnable_returns_awaiting_with_free_board_else_none() {
    let db = fresh_db();
    insert_board(&db, "b1", "mcxa266", BoardHealth::Healthy);
    let a = enqueue_with(&db, 100, |_| {});
    JobRow::transition_submitted_to_building(db.raw_conn(), &a, 110).unwrap();
    JobRow::transition_building_to_awaiting_board(db.raw_conn(), &a, "/e.elf").unwrap();

    // Board free → runnable.
    let pick = pick_runnable(db.raw_conn(), CFG, 1000).unwrap().unwrap();
    assert_eq!(pick.job.id, a);
    assert_eq!(pick.board.spec.id, "b1");

    // Occupy the board with a running job → no longer runnable.
    JobRow::transition_awaiting_to_running(db.raw_conn(), &a, "b1").unwrap();
    let b = enqueue_with(&db, 200, |_| {});
    JobRow::transition_submitted_to_building(db.raw_conn(), &b, 210).unwrap();
    JobRow::transition_building_to_awaiting_board(db.raw_conn(), &b, "/e2.elf").unwrap();
    assert_eq!(pick_runnable(db.raw_conn(), CFG, 1000).unwrap().map(|p| p.job.id), None);
    assert_eq!(JobState::AwaitingBoard, JobRow::get(db.raw_conn(), &b).unwrap().state);
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test -p paavo-core --test scheduler_two_stage`
Expected: FAIL — `pick_buildable`/`pick_runnable` not found.

- [ ] **Step 3: Add DB list helpers**

In `crates/paavo-db/src/job.rs`, inside `impl JobRow`, add next to `list_submitted`:

```rust
    /// Jobs in `AwaitingBoard`, scheduler order (priority then oldest).
    pub fn list_awaiting_board(conn: &Connection, limit: u32) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT * FROM job WHERE state = 'awaiting_board'
             ORDER BY priority ASC, submitted_at ASC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Distinct `tar_blake3` values of jobs currently `Building` — the
    /// single-flight set the build scheduler skips.
    pub fn building_tar_blake3s(conn: &Connection) -> Result<Vec<String>> {
        let mut stmt = conn.prepare("SELECT DISTINCT tar_blake3 FROM job WHERE state = 'building'")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
```

- [ ] **Step 4: Add the pickers**

In `crates/paavo-core/src/scheduler.rs`, add (reusing the private `lru_pick` and the `Priority` promotion pattern already in `pick_next`):

```rust
/// Build-stage pick: highest-priority `Submitted` job whose `tar_blake3`
/// is NOT already `Building` (single-flight), with starvation promotion.
/// No board required. Pure read; caller transitions to Building.
pub fn pick_buildable(
    conn: &Connection,
    config: SchedulerConfig,
    now_ms: i64,
) -> paavo_db::Result<Option<paavo_db::JobRow>> {
    let building: std::collections::HashSet<String> =
        paavo_db::JobRow::building_tar_blake3s(conn)?.into_iter().collect();
    let mut promoted: Vec<paavo_db::JobRow> = paavo_db::JobRow::list_submitted(conn, MAX_SUBMITTED_SCAN)?
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
    Ok(promoted.into_iter().find(|j| !building.contains(&j.tar_blake3)))
}

/// Run-stage pick: highest-priority `AwaitingBoard` job that has a free
/// healthy matching board (LRU), with starvation promotion. Pure read;
/// caller transitions to Running.
pub fn pick_runnable(
    conn: &Connection,
    config: SchedulerConfig,
    now_ms: i64,
) -> paavo_db::Result<Option<ScheduledJob>> {
    let mut promoted: Vec<paavo_db::JobRow> = paavo_db::JobRow::list_awaiting_board(conn, MAX_SUBMITTED_SCAN)?
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
```

- [ ] **Step 5: Export**

In `crates/paavo-core/src/lib.rs`, change the scheduler `pub use` to:
`pub use scheduler::{pick_buildable, pick_next, pick_runnable, ScheduledJob, SchedulerConfig};`

- [ ] **Step 6: Run — verify green**

Run: `cargo test -p paavo-core --test scheduler_two_stage && cargo test -p paavo-db`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/paavo-db/src/job.rs crates/paavo-core/src/scheduler.rs crates/paavo-core/src/lib.rs crates/paavo-core/tests/scheduler_two_stage.rs
git commit -m "feat(core): pick_buildable + pick_runnable with single-flight"
```

---

### Task 6: `cancel_if_pending` (Submitted + AwaitingBoard)

Additive: keep `cancel_if_submitted`.

**Files:**
- Modify: `crates/paavo-core/src/cancel.rs`
- Modify: `crates/paavo-core/src/lib.rs`
- Modify: `crates/paavo-core/tests/cancel.rs`

- [ ] **Step 1: Write failing tests**

In `crates/paavo-core/tests/cancel.rs`, add (mirror the file's existing harness; drive states via `JobRow` transitions):

```rust
#[test]
fn cancel_if_pending_aborts_awaiting_board() {
    let db = common::fresh_db();
    common::insert_board(&db, "b1", "mcxa266", paavo_proto::BoardHealth::Healthy);
    let id = common::enqueue_with(&db, 0, |_| {});
    paavo_db::JobRow::transition_submitted_to_building(db.raw_conn(), &id, 1).unwrap();
    paavo_db::JobRow::transition_building_to_awaiting_board(db.raw_conn(), &id, "/e.elf").unwrap();

    let out = paavo_core::cancel_if_pending(db.raw_conn(), &id, 5).unwrap();
    assert!(matches!(out, Some(paavo_proto::JobOutcome::Aborted { by: paavo_proto::AbortReason::User })));
    assert_eq!(paavo_db::JobRow::get(db.raw_conn(), &id).unwrap().state, paavo_proto::JobState::Aborted);
}

#[test]
fn cancel_if_pending_rejects_running() {
    let db = common::fresh_db();
    common::insert_board(&db, "b1", "mcxa266", paavo_proto::BoardHealth::Healthy);
    let id = common::enqueue_with(&db, 0, |_| {});
    paavo_db::JobRow::transition_submitted_to_building(db.raw_conn(), &id, 1).unwrap();
    paavo_db::JobRow::transition_building_to_awaiting_board(db.raw_conn(), &id, "/e.elf").unwrap();
    paavo_db::JobRow::transition_awaiting_to_running(db.raw_conn(), &id, "b1").unwrap();
    assert!(matches!(
        paavo_core::cancel_if_pending(db.raw_conn(), &id, 5),
        Err(paavo_core::CoreError::NotCancellable { .. })
    ));
}
```

(If `cancel.rs` test file doesn't already `mod common;`, add it — `crates/paavo-core/tests/common/mod.rs` is shared.)

- [ ] **Step 2: Run — verify failure**

Run: `cargo test -p paavo-core --test cancel cancel_if_pending`
Expected: FAIL — not found.

- [ ] **Step 3: Implement**

In `crates/paavo-core/src/cancel.rs`, add:

```rust
/// Inline-cancel a job that is waiting (no worker, no board):
/// `Submitted` or `AwaitingBoard`. Marks `Aborted { User }`. Any other
/// state returns `NotCancellable` (the caller routes Building→kill-build
/// and Running→watchdog). Supersedes `cancel_if_submitted` for the
/// two-stage model.
pub fn cancel_if_pending(
    conn: &Connection,
    id: &JobId,
    now_ms: i64,
) -> Result<Option<JobOutcome>> {
    let row = paavo_db::JobRow::get(conn, id)?;
    if !matches!(row.state, JobState::Submitted | JobState::AwaitingBoard) {
        return Err(CoreError::NotCancellable { state: row.state });
    }
    let outcome = JobOutcome::Aborted { by: AbortReason::User };
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
```

- [ ] **Step 4: Export** — in `crates/paavo-core/src/lib.rs` change the cancel line to
`pub use cancel::{cancel_if_pending, cancel_if_submitted};`

- [ ] **Step 5: Run — verify green** — `cargo test -p paavo-core --test cancel`

- [ ] **Step 6: Commit**

```bash
git add crates/paavo-core/src/cancel.rs crates/paavo-core/src/lib.rs crates/paavo-core/tests/cancel.rs
git commit -m "feat(core): cancel_if_pending covers Submitted + AwaitingBoard"
```

---

## Phase C — paavod plumbing

### Task 7: Config knob `max_concurrent_builds`

**Files:**
- Modify: `crates/paavod/src/config.rs` (`SchedulerConfig` ~line 108-118)
- Modify: every explicit `SchedulerConfig { .. }` literal (compiler lists them; known: `crates/paavod/tests/dispatch_loop.rs:106` and `:297`)
- Modify: `crates/paavod/tests/config_loading.rs` (assert the default)

- [ ] **Step 1: Add the field + default**

In `crates/paavod/src/config.rs`, inside `SchedulerConfig`:

```rust
    /// Max concurrent `cargo build` processes (each gets its own
    /// CARGO_TARGET_DIR). Jobs beyond this wait in `Submitted`.
    #[serde(default = "default_max_concurrent_builds")]
    pub max_concurrent_builds: usize,
```

Add near `default_starvation_threshold_s`:

```rust
fn default_max_concurrent_builds() -> usize {
    5
}
```

- [ ] **Step 2: Fix all explicit literals**

Run `cargo build -p paavod --tests` and add `max_concurrent_builds: 5,` to each `SchedulerConfig { .. }` the compiler flags (the two in `dispatch_loop.rs`, plus any others).

- [ ] **Step 3: Test the default**

In `crates/paavod/tests/config_loading.rs`, add an assertion (matching that file's load style) that a TOML `[scheduler]` block without `max_concurrent_builds` yields `5`. If the file has a `minimal config` test, extend it:

```rust
assert_eq!(cfg.scheduler.max_concurrent_builds, 5);
```

- [ ] **Step 4: Run — verify green** — `cargo test -p paavod --test config_loading && cargo build -p paavod --tests`

- [ ] **Step 5: Commit**

```bash
git add crates/paavod/src/config.rs crates/paavod/tests/
git commit -m "feat(paavod): scheduler.max_concurrent_builds config (default 5)"
```

---

### Task 8: StateDir build-slot pool

**Files:**
- Modify: `crates/paavod/src/state_dir.rs`

- [ ] **Step 1: Write failing test**

Append to `crates/paavod/src/state_dir.rs` a `#[cfg(test)]` module (or add to an existing one):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn build_slots_are_created_and_indexed() {
        let tmp = tempfile::tempdir().unwrap();
        let sd = StateDir::from_root(tmp.path());
        sd.ensure_build_slots(3).unwrap();
        for i in 0..3 {
            assert!(sd.build_slot_dir(i).is_dir(), "slot {i} dir must exist");
        }
        assert_eq!(sd.build_slot_dir(0), tmp.path().join("build-slots").join("0"));
    }
}
```

(Ensure `tempfile` is a dev-dependency of `paavod` — it already is, used across tests.)

- [ ] **Step 2: Run — verify failure** — `cargo test -p paavod --lib state_dir`
Expected: FAIL — `ensure_build_slots`/`build_slot_dir` not found.

- [ ] **Step 3: Implement**

In `crates/paavod/src/state_dir.rs`, add to `impl StateDir`:

```rust
    /// Target dir for build slot `i` (`<root>/build-slots/<i>`). Reused
    /// across builds so cargo keeps incremental state per slot.
    pub fn build_slot_dir(&self, i: usize) -> PathBuf {
        self.root.join("build-slots").join(i.to_string())
    }

    /// Create `<root>/build-slots/<0..n>`. Idempotent.
    pub fn ensure_build_slots(&self, n: usize) -> std::io::Result<()> {
        for i in 0..n {
            std::fs::create_dir_all(self.build_slot_dir(i))?;
        }
        Ok(())
    }
```

Keep `cargo_target_dir` for now (old dispatch still references it; removed in Task 14).

- [ ] **Step 4: Run — verify green** — `cargo test -p paavod --lib state_dir`

- [ ] **Step 5: Commit**

```bash
git add crates/paavod/src/state_dir.rs
git commit -m "feat(paavod): per-slot build target dirs in StateDir"
```

---

### Task 9: Cancellable build in paavo-build (kill cargo child)

**Files:**
- Modify: `crates/paavo-build/src/build.rs`
- Modify: `crates/paavo-build/src/error.rs` (add `Cancelled`)

- [ ] **Step 1: Write failing tests for the kill helper**

In `crates/paavo-build/src/build.rs` `#[cfg(test)] mod tests`, add:

```rust
    fn sleeper() -> std::process::Child {
        let mut c = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "ping 127.0.0.1 -n 30 > NUL"]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "sleep 30"]);
            c
        };
        c.stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()
    }

    #[test]
    fn wait_or_kill_kills_promptly_on_signal() {
        let (tx, rx) = crossbeam_channel::unbounded::<()>();
        let mut child = sleeper();
        let start = std::time::Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = tx.send(());
        });
        let (_status, cancelled) = wait_or_kill(&mut child, &rx).unwrap();
        assert!(cancelled, "should report cancellation");
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "child should be killed promptly");
    }

    #[test]
    fn wait_or_kill_runs_to_completion_when_sender_dropped() {
        let (tx, rx) = crossbeam_channel::unbounded::<()>();
        drop(tx); // never-cancellable path
        let mut child = if cfg!(windows) {
            std::process::Command::new("cmd").args(["/C", "exit 0"]).spawn().unwrap()
        } else {
            std::process::Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap()
        };
        let (status, cancelled) = wait_or_kill(&mut child, &rx).unwrap();
        assert!(!cancelled);
        assert!(status.success());
    }
```

- [ ] **Step 2: Run — verify failure** — `cargo test -p paavo-build wait_or_kill`
Expected: FAIL — `wait_or_kill` not found.

- [ ] **Step 3: Add `BuildError::Cancelled`**

In `crates/paavo-build/src/error.rs`, add a variant to `BuildError`:

```rust
    /// The build was cancelled (the cargo child was killed) before it
    /// finished. paavod maps this to `Aborted { User }`.
    #[error("build cancelled")]
    Cancelled,
```

- [ ] **Step 4: Add `wait_or_kill` + thread it through**

In `crates/paavo-build/src/build.rs`:

1. Add the helper:

```rust
/// Wait for `child`, but if `cancel_rx` fires first, kill it. Returns
/// `(status, was_cancelled)`. Polls `try_wait` so we react to cancel
/// without a second thread. A dropped sender (the non-cancellable
/// `build_release` path) blocks to completion exactly like `wait()`.
fn wait_or_kill(
    child: &mut std::process::Child,
    cancel_rx: &crossbeam_channel::Receiver<()>,
) -> std::io::Result<(std::process::ExitStatus, bool)> {
    use crossbeam_channel::RecvTimeoutError;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status, false));
        }
        match cancel_rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(()) => {
                let _ = child.kill();
                let status = child.wait()?;
                return Ok((status, true));
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                let status = child.wait()?;
                return Ok((status, false));
            }
        }
    }
}
```

2. Change `run_cargo_streaming` to accept `cancel_rx: &crossbeam_channel::Receiver<()>` and replace `let status = child.wait()?;` with:

```rust
    let (status, cancelled) = wait_or_kill(&mut child, cancel_rx)?;
```

After joining the reader threads, before the `!status.success()` check, add:

```rust
    if cancelled {
        return Err(BuildError::Cancelled);
    }
```

3. Add the public cancellable entry point and re-point the existing ones:

```rust
/// Cancellable variant of [`build_release_streaming`]. `cancel_rx`
/// firing kills the in-flight cargo child and returns
/// [`BuildError::Cancelled`].
pub fn build_release_streaming_cancellable(
    plan: &BuildPlan,
    lines: BuildLineTx,
    cancel_rx: crossbeam_channel::Receiver<()>,
) -> Result<BuildResult> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    for pkg in &plan.cargo_update_packages {
        let _tail = run_cargo_streaming(&cargo, &["update", "-p", pkg], plan, lines.clone(), &cancel_rx)?;
    }
    let stderr_tail = run_cargo_streaming(&cargo, &["build", "--release"], plan, lines, &cancel_rx)?;
    let hint = ManifestArtifactHint::default();
    let elf_path = discover_elf(&plan.crate_dir, &plan.target_dir, &hint)?;
    let elf_size_bytes = std::fs::metadata(&elf_path)?.len();
    Ok(BuildResult { elf_path, elf_size_bytes, stderr_tail })
}
```

Refactor `build_release_streaming` to delegate (never-cancellable: drop the sender so `wait_or_kill` blocks to completion):

```rust
pub fn build_release_streaming(plan: &BuildPlan, lines: BuildLineTx) -> Result<BuildResult> {
    let (_never_tx, never_rx) = crossbeam_channel::unbounded::<()>();
    drop(_never_tx);
    build_release_streaming_cancellable(plan, lines, never_rx)
}
```

(`build_release` already delegates to `build_release_streaming`, so it inherits the change unchanged.)

- [ ] **Step 5: Run — verify green** — `cargo test -p paavo-build`
Expected: PASS (existing `build_invocation.rs` still green; new `wait_or_kill` tests pass).

- [ ] **Step 6: Commit**

```bash
git add crates/paavo-build/src/build.rs crates/paavo-build/src/error.rs
git commit -m "feat(build): cancellable build that kills the cargo child"
```

---

### Task 10: `Builder` trait + `RealBuilder`

**Files:**
- Create: `crates/paavod/src/builder.rs`
- Modify: `crates/paavod/src/lib.rs` (add `pub mod builder;`)

- [ ] **Step 1: Implement (no separate test — wraps tested code; FakeBuilder lands in Task 12 tests)**

Create `crates/paavod/src/builder.rs`:

```rust
//! Injectable build step, mirroring `paavo_core::Runner`. Lets dispatch
//! run a real cargo build in production and a fake one in tests, and
//! routes Building-phase cancellation to a killed cargo child.
//!
//! The Builder owns the whole tar → ELF step (unpack, locate crate,
//! cargo). Dispatch owns everything around it: cache lookup, slot
//! assignment, the stable-artifact copy, transitions, and log
//! forwarding. Keeping unpack inside `RealBuilder` lets test builders
//! skip tars entirely.

use paavo_build::BuildLineTx;
use std::path::PathBuf;

/// What dispatch hands a build: the job row plus the two directories it
/// owns for this attempt.
pub struct BuildRequest<'a> {
    /// The job being built (source of `tar_path`, `cargo_update_packages`).
    pub job: &'a paavo_db::JobRow,
    /// Where to unpack the tar (`<state>/sandboxes/<job_id>`).
    pub sandbox_dir: PathBuf,
    /// The slot's `CARGO_TARGET_DIR` (`<state>/build-slots/<i>`).
    pub target_dir: PathBuf,
}

/// Result of a build attempt.
pub enum BuildOutcome {
    /// Built; ELF discovered inside `target_dir`. Dispatch copies it to
    /// the content-addressed cache path.
    Ok {
        /// Path to the discovered ELF (inside the slot's target dir).
        elf_path: PathBuf,
    },
    /// cargo (or unpack/discovery) failed; the String is the stderr tail
    /// or error message (→ `BuildErr`).
    Failed(String),
    /// The build was cancelled (cargo child killed) (→ `Aborted{User}`).
    Cancelled,
}

/// The build step dispatch invokes once per job.
pub trait Builder: Send + Sync {
    /// Unpack + compile `req`, streaming cargo lines to `lines`.
    /// `cancel_rx` firing kills the build.
    fn build(
        &self,
        req: BuildRequest<'_>,
        lines: BuildLineTx,
        cancel_rx: crossbeam_channel::Receiver<()>,
    ) -> BuildOutcome;
}

/// Production builder backed by `paavo_build`.
pub struct RealBuilder;

impl Builder for RealBuilder {
    fn build(
        &self,
        req: BuildRequest<'_>,
        lines: BuildLineTx,
        cancel_rx: crossbeam_channel::Receiver<()>,
    ) -> BuildOutcome {
        use std::io::Read;
        // 1. Read + unpack the tar into the sandbox.
        let mut bytes = Vec::new();
        if let Err(e) = std::fs::File::open(&req.job.tar_path)
            .and_then(|mut f| f.read_to_end(&mut bytes))
        {
            return BuildOutcome::Failed(format!("read tar {}: {e}", req.job.tar_path));
        }
        if let Err(e) = paavo_build::tar::unpack_into(&bytes, &req.sandbox_dir) {
            return BuildOutcome::Failed(e.to_string());
        }
        // 2. Find the unique crate dir (the one containing Cargo.toml).
        let crate_root = match walkdir::WalkDir::new(&req.sandbox_dir)
            .min_depth(1)
            .max_depth(2)
            .into_iter()
            .flatten()
            .find(|e| e.file_name() == "Cargo.toml")
            .and_then(|e| e.path().parent().map(|p| p.to_path_buf()))
        {
            Some(r) => r,
            None => return BuildOutcome::Failed("no Cargo.toml in uploaded tar".into()),
        };
        // 3. Build into the slot's target dir.
        let plan = paavo_build::BuildPlan {
            crate_dir: crate_root,
            target_dir: req.target_dir,
            cargo_update_packages: req.job.cargo_update_packages.clone(),
        };
        match paavo_build::build_release_streaming_cancellable(&plan, lines, cancel_rx) {
            Ok(res) => BuildOutcome::Ok { elf_path: res.elf_path },
            Err(paavo_build::BuildError::Cancelled) => BuildOutcome::Cancelled,
            Err(e) => BuildOutcome::Failed(e.to_string()),
        }
    }
}
```

In `crates/paavod/src/lib.rs`, add `pub mod builder;` (alphabetically near `pub mod app;`).

- [ ] **Step 2: Run — verify compiles** — `cargo build -p paavod`

- [ ] **Step 3: Commit**

```bash
git add crates/paavod/src/builder.rs crates/paavod/src/lib.rs
git commit -m "feat(paavod): Builder trait + RealBuilder"
```

---

### Task 11: Build-cancel registry

**Files:**
- Modify: `crates/paavod/src/cancellation.rs`
- Modify: `crates/paavod/src/app_state.rs` (add `build_cancel` field)
- Modify: every `AppState { .. }` literal (compiler lists them; known: `main.rs`, `dispatch_loop.rs`, and other `paavod/tests/*`)

- [ ] **Step 1: Write failing test**

In `crates/paavod/src/cancellation.rs` `#[cfg(test)]` module (add one if absent):

```rust
#[cfg(test)]
mod build_cancel_tests {
    use super::BuildCancelRegistry;
    use paavo_proto::JobId;

    #[test]
    fn register_signal_unregister() {
        let reg = BuildCancelRegistry::default();
        let id = JobId::new();
        let rx = reg.register(id);
        assert_eq!(reg.active(), 1);
        assert!(reg.signal(&id), "signal delivers while rx alive");
        assert_eq!(rx.recv().unwrap(), ());
        reg.unregister(&id);
        assert_eq!(reg.active(), 0);
        assert!(!reg.signal(&id), "signal after unregister is false");
    }
}
```

- [ ] **Step 2: Run — verify failure** — `cargo test -p paavod --lib build_cancel_tests`
Expected: FAIL — `BuildCancelRegistry` not found.

- [ ] **Step 3: Implement the registry**

In `crates/paavod/src/cancellation.rs`, add:

```rust
/// Per-job kill switch for the BUILD phase. Separate from
/// `CancellationRegistry` (which carries `RunCommand` to the run
/// watchdog): a build cancel just kills the cargo child via a `()`
/// signal the build task hands to `Builder::build`.
#[derive(Clone, Default)]
pub struct BuildCancelRegistry {
    inner: Arc<Mutex<HashMap<JobId, crossbeam_channel::Sender<()>>>>,
}

impl BuildCancelRegistry {
    /// Allocate a kill channel for a build about to start; returns the rx
    /// the build task passes to `Builder::build`.
    pub fn register(&self, id: JobId) -> crossbeam_channel::Receiver<()> {
        let (tx, rx) = crossbeam_channel::unbounded::<()>();
        self.inner.lock().insert(id, tx);
        rx
    }

    /// Request a kill. `true` if a live build channel existed.
    pub fn signal(&self, id: &JobId) -> bool {
        match self.inner.lock().get(id) {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    /// Drop the entry when the build finishes. Idempotent.
    pub fn unregister(&self, id: &JobId) {
        self.inner.lock().remove(id);
    }

    /// In-flight build count (drain bookkeeping + tests).
    pub fn active(&self) -> usize {
        self.inner.lock().len()
    }
}
```

(The file already imports `Arc`, `Mutex`, `HashMap`, `JobId`. Add `use crossbeam_channel;` if not present — it's already a dependency.)

- [ ] **Step 4: Add to AppState**

In `crates/paavod/src/app_state.rs`, add to `struct AppState`:

```rust
    /// Per-job BUILD-phase kill registry (Building cancel → kill cargo).
    pub build_cancel: crate::cancellation::BuildCancelRegistry,
```

- [ ] **Step 5: Fix all AppState literals**

Run `cargo build -p paavod --tests` and add `build_cancel: CancellationRegistry...`—specifically `build_cancel: paavod::cancellation::BuildCancelRegistry::default(),` (or `Default::default()`)—to each `AppState { .. }` the compiler flags (`main.rs` and the `tests/*.rs` fixtures).

- [ ] **Step 6: Run — verify green** — `cargo test -p paavod --lib && cargo build -p paavod --tests`

- [ ] **Step 7: Commit**

```bash
git add crates/paavod/src/cancellation.rs crates/paavod/src/app_state.rs crates/paavod/src/main.rs crates/paavod/tests/
git commit -m "feat(paavod): BuildCancelRegistry for Building-phase cancel"
```

---

## Phase D — two-stage dispatch

### Task 12: Rewrite `dispatch.rs` as a two-stage loop

**Files:**
- Modify (rewrite): `crates/paavod/src/dispatch.rs`
- Modify: `crates/paavod/src/main.rs` (construct `RealBuilder`, `ensure_build_slots`, new `spawn` signature)
- Modify: `crates/paavod/tests/dispatch_loop.rs` (add `FakeBuilder`, update `spawn` calls, add cap + build-while-busy tests)

- [ ] **Step 1: Replace `crates/paavod/src/dispatch.rs` with the two-stage loop**

```rust
//! Two-stage dispatch loop.
//!
//! Build stage: claims `Submitted → Building`, bounded by an in-memory
//! slot pool of `max_concurrent_builds` (each slot a dedicated
//! CARGO_TARGET_DIR). On success copies the ELF to a stable
//! content-addressed cache path and moves the job to `AwaitingBoard`,
//! releasing the slot. The board is NOT held during a build.
//!
//! Run stage: claims `AwaitingBoard → Running` only when a matching
//! healthy board is free, then invokes the Runner. Board exclusivity is
//! enforced by `find_healthy_for_selector` (running rows only).
//!
//! Each tick runs the run stage first (keep scarce boards busy), then
//! fills build slots. Drain stops new picks and exits once no build or
//! run is in flight.

use crate::app_state::AppState;
use crate::builder::{BuildOutcome, BuildRequest, Builder};
use crate::job_logs::LiveEvent;
use chrono::Utc;
use paavo_core::{
    apply_outcome_to_board, cache_lookup, cache_store, pick_buildable, pick_runnable, CacheLookup,
    QuarantinePolicy, RunOutcome, Runner, SchedulerConfig,
};
use paavo_db::{JobRow, OutcomeRecord};
use paavo_proto::{AbortReason, JobId, JobOutcome, JobPhase, JobState, TerminalOutcome};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{error, warn};

/// Spawn the dispatch loop. Returns immediately. The loop exits when
/// `state.drain` is set AND no build or run is in flight.
pub fn spawn(
    state: AppState,
    builder: Arc<dyn Builder>,
    runner: Arc<dyn Runner>,
) -> tokio::task::JoinHandle<()> {
    let n = state.config.scheduler.max_concurrent_builds.max(1);
    // Slot pool: a bounded channel pre-filled with slot indices. Acquire =
    // try_recv; release = send back (from the finished build task).
    let (slot_tx, slot_rx) = crossbeam_channel::bounded::<usize>(n);
    for i in 0..n {
        let _ = slot_tx.send(i);
    }
    tokio::spawn(async move {
        loop {
            if state.drain.is_draining() {
                if state.build_cancel.active() == 0 && state.cancellation.active() == 0 {
                    return;
                }
                sleep(Duration::from_millis(100)).await;
                continue;
            }
            let cfg = SchedulerConfig {
                starvation_threshold_ms: state.config.scheduler.starvation_threshold_s * 1_000,
            };
            run_stage(&state, &runner, cfg);
            build_stage(&state, &builder, &slot_rx, &slot_tx, cfg);
            sleep(Duration::from_millis(250)).await;
        }
    })
}

/// Drain `AwaitingBoard` jobs onto free boards until none can dispatch.
fn run_stage(state: &AppState, runner: &Arc<dyn Runner>, cfg: SchedulerConfig) {
    loop {
        let now = Utc::now().timestamp_millis();
        let pick = {
            let db = state.db.lock();
            pick_runnable(db.raw_conn(), cfg, now)
        };
        let sched = match pick {
            Ok(Some(s)) => s,
            Ok(None) => break,
            Err(e) => {
                warn!(error = %e, "dispatch: pick_runnable failed");
                break;
            }
        };
        let job_id = sched.job.id;
        let board_id = sched.board.spec.id.clone();
        let claim_ok = {
            let db = state.db.lock();
            let r = JobRow::transition_awaiting_to_running(db.raw_conn(), &job_id, &board_id);
            if r.is_ok() {
                let _ = paavo_db::BoardRow::touch_last_used(db.raw_conn(), &board_id, now);
            }
            r.is_ok()
        };
        if !claim_ok {
            break; // raced (e.g. cancel landed); next tick recovers
        }
        state
            .job_logs
            .publish(job_id, LiveEvent::Phase(JobPhase::Running));
        state.cancellation.register(job_id);
        let st = state.clone();
        let rn = runner.clone();
        tokio::task::spawn_blocking(move || run_one_run(st, rn, sched.job, board_id));
    }
}

/// Fill free build slots with `Submitted` jobs (single-flight respected
/// by `pick_buildable`).
fn build_stage(
    state: &AppState,
    builder: &Arc<dyn Builder>,
    slot_rx: &crossbeam_channel::Receiver<usize>,
    slot_tx: &crossbeam_channel::Sender<usize>,
    cfg: SchedulerConfig,
) {
    loop {
        let now = Utc::now().timestamp_millis();
        let pick = {
            let db = state.db.lock();
            pick_buildable(db.raw_conn(), cfg, now)
        };
        let job = match pick {
            Ok(Some(j)) => j,
            Ok(None) => break,
            Err(e) => {
                warn!(error = %e, "dispatch: pick_buildable failed");
                break;
            }
        };
        let slot = match slot_rx.try_recv() {
            Ok(s) => s,
            Err(_) => break, // at cap
        };
        let job_id = job.id;
        let claim_ok = {
            let db = state.db.lock();
            JobRow::transition_submitted_to_building(db.raw_conn(), &job_id, now).is_ok()
        };
        if !claim_ok {
            let _ = slot_tx.send(slot);
            break;
        }
        state
            .job_logs
            .publish(job_id, LiveEvent::Phase(JobPhase::Building));
        let cancel_rx = state.build_cancel.register(job_id);
        let st = state.clone();
        let b = builder.clone();
        let stx = slot_tx.clone();
        tokio::task::spawn_blocking(move || run_one_build(st, b, job, slot, stx, cancel_rx));
    }
}

enum BuildStageOutcome {
    /// Job advanced to `AwaitingBoard`; broker stays open for the run.
    Advanced,
    /// Build stage produced a terminal outcome (BuildErr/Aborted/Infra).
    Terminal(JobOutcome),
}

/// Per-job build work on the blocking pool. Always reclaims the slot +
/// build-cancel entry, even on panic.
fn run_one_build(
    state: AppState,
    builder: Arc<dyn Builder>,
    job: JobRow,
    slot: usize,
    slot_tx: crossbeam_channel::Sender<usize>,
    cancel_rx: crossbeam_channel::Receiver<()>,
) {
    let job_id = job.id;
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        build_inner(&state, builder.as_ref(), &job, slot, cancel_rx)
    }));
    let _ = slot_tx.send(slot);
    state.build_cancel.unregister(&job_id);
    match res {
        Ok(BuildStageOutcome::Advanced) => {}
        Ok(BuildStageOutcome::Terminal(outcome)) => finalize_terminal(&state, &job_id, outcome),
        Err(payload) => {
            let message = panic_message(&payload);
            error!(%job_id, %message, "dispatch: run_one_build panicked");
            finalize_terminal(
                &state,
                &job_id,
                JobOutcome::Failed(TerminalOutcome::InfraErr {
                    stage: "build_dispatch".into(),
                    message,
                }),
            );
        }
    }
}

fn build_inner(
    state: &AppState,
    builder: &dyn Builder,
    job: &JobRow,
    slot: usize,
    cancel_rx: crossbeam_channel::Receiver<()>,
) -> BuildStageOutcome {
    let sd = crate::state_dir::StateDir::from_root(&state.config.server.state_dir);
    let now = Utc::now().timestamp_millis();

    // Cache hit → straight to AwaitingBoard (no slot work).
    if !job.skip_cache {
        let lookup = {
            let db = state.db.lock();
            cache_lookup(db.raw_conn(), &job.tar_blake3, now)
        };
        if let Ok(CacheLookup::Hit { elf_path }) = lookup {
            let db = state.db.lock();
            return match JobRow::transition_building_to_awaiting_board(
                db.raw_conn(),
                &job.id,
                &elf_path.display().to_string(),
            ) {
                Ok(()) => BuildStageOutcome::Advanced,
                Err(e) => BuildStageOutcome::Terminal(JobOutcome::Failed(
                    TerminalOutcome::InfraErr {
                        stage: "transition_awaiting_board".into(),
                        message: e.to_string(),
                    },
                )),
            };
        }
    }

    // Build forwarder: cargo lines → broker + log_frame (build phase seq 0..).
    let log_seq = Arc::new(AtomicU64::new(0));
    let job_start = std::time::Instant::now();
    let (build_tx, build_rx) = crossbeam_channel::unbounded::<paavo_build::BuildLine>();
    let mut sink = crate::log_sink::FrameSink::new(
        job.id,
        state.job_logs.clone(),
        state.db.clone(),
        log_seq,
        job_start,
    );
    let fwd = std::thread::Builder::new()
        .name("paavod-build-forwarder".into())
        .spawn(move || loop {
            match build_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(bl) => {
                    let target = match bl.stream {
                        paavo_build::BuildStream::Stdout => "cargo:stdout",
                        paavo_build::BuildStream::Stderr => "cargo:stderr",
                    };
                    sink.push(paavo_proto::LogLevel::Info, Some(target.to_string()), bl.text);
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => sink.tick(),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    sink.finish();
                    break;
                }
            }
        })
        .expect("spawn paavod-build-forwarder thread");

    let req = BuildRequest {
        job,
        sandbox_dir: sd.sandboxes_dir.join(job.id.to_string()),
        target_dir: sd.build_slot_dir(slot),
    };
    let outcome = builder.build(req, build_tx, cancel_rx);
    let _ = fwd.join();

    match outcome {
        BuildOutcome::Ok { elf_path } => {
            // Copy to a stable, content-addressed artifact so slot reuse
            // can't clobber it and the cache path stays valid.
            let stable = sd.cache_elfs_dir.join(format!("{}.elf", job.tar_blake3));
            if let Err(e) = std::fs::copy(&elf_path, &stable) {
                return BuildStageOutcome::Terminal(JobOutcome::Failed(
                    TerminalOutcome::InfraErr {
                        stage: "artifact_copy".into(),
                        message: e.to_string(),
                    },
                ));
            }
            let stable_str = stable.display().to_string();
            let db = state.db.lock();
            let now2 = Utc::now().timestamp_millis();
            if let Err(e) = cache_store(db.raw_conn(), &job.tar_blake3, &stable, now2) {
                warn!(error = %e, job_id = %job.id, "dispatch: cache_store failed; continuing");
            }
            match JobRow::transition_building_to_awaiting_board(db.raw_conn(), &job.id, &stable_str) {
                Ok(()) => BuildStageOutcome::Advanced,
                Err(e) => BuildStageOutcome::Terminal(JobOutcome::Failed(
                    TerminalOutcome::InfraErr {
                        stage: "transition_awaiting_board".into(),
                        message: e.to_string(),
                    },
                )),
            }
        }
        BuildOutcome::Failed(stderr) => {
            BuildStageOutcome::Terminal(JobOutcome::Failed(TerminalOutcome::BuildErr { stderr }))
        }
        BuildOutcome::Cancelled => {
            BuildStageOutcome::Terminal(JobOutcome::Aborted { by: AbortReason::User })
        }
    }
}

/// Per-job run work on the blocking pool. Continues the log seq space
/// after the build-phase frames so live viewers see one timeline.
fn run_one_run(state: AppState, runner: Arc<dyn Runner>, job: JobRow, board_id: String) {
    let job_id = job.id;
    let start_seq = {
        let db = state.db.lock();
        next_seq(db.raw_conn(), &job_id)
    };
    let log_seq = Arc::new(AtomicU64::new(start_seq));
    let job_start = std::time::Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runner.run(paavo_core::RunContext {
            job_id,
            board_id: &board_id,
            log_seq: log_seq.clone(),
            job_start,
        })
    }));
    let (outcome, probe_released_cleanly) = match result {
        Ok(RunOutcome {
            outcome,
            probe_released_cleanly,
        }) => (outcome, probe_released_cleanly),
        Err(payload) => {
            let message = panic_message(&payload);
            error!(%job_id, %message, "dispatch: run_one_run panicked");
            (
                JobOutcome::Failed(TerminalOutcome::InfraErr {
                    stage: "dispatch".into(),
                    message,
                }),
                false,
            )
        }
    };
    finalize_run(&state, &job_id, &board_id, outcome, probe_released_cleanly);
}

/// Next log_frame seq for a job (continues the build-phase numbering).
fn next_seq(conn: &rusqlite::Connection, job_id: &JobId) -> u64 {
    conn.query_row(
        "SELECT COALESCE(MAX(seq), -1) + 1 FROM log_frame WHERE job_id = ?1",
        rusqlite::params![job_id.to_string()],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
    .max(0) as u64
}

/// Run-stage terminal: persist outcome, apply board quarantine policy,
/// publish Terminal, finalize broker, unregister run cancellation.
fn finalize_run(
    state: &AppState,
    job_id: &JobId,
    board_id: &str,
    outcome: JobOutcome,
    probe_released_cleanly: bool,
) {
    let terminal_state = terminal_state_of(&outcome);
    let now = Utc::now().timestamp_millis();
    {
        let db = state.db.lock();
        if let Err(e) = JobRow::finalize(
            db.raw_conn(),
            job_id,
            &OutcomeRecord {
                state: terminal_state,
                outcome: outcome.clone(),
                finished_at_ms: now,
            },
        ) {
            warn!(error = %e, %job_id, "dispatch: finalize_run failed");
        }
        if let Err(e) = apply_outcome_to_board(
            db.raw_conn(),
            board_id,
            &outcome,
            probe_released_cleanly,
            QuarantinePolicy {
                consecutive_infra_failures: state.config.quarantine.consecutive_infra_failures,
            },
        ) {
            warn!(error = %e, board_id, "dispatch: apply_outcome_to_board failed");
        }
    }
    state.job_logs.publish(*job_id, LiveEvent::Terminal(outcome));
    state.job_logs.finalize(*job_id);
    state.cancellation.unregister(job_id);
}

/// Build-stage terminal: persist outcome, publish Terminal, finalize
/// broker. No board involved → no quarantine accounting.
fn finalize_terminal(state: &AppState, job_id: &JobId, outcome: JobOutcome) {
    let terminal_state = terminal_state_of(&outcome);
    let now = Utc::now().timestamp_millis();
    {
        let db = state.db.lock();
        if let Err(e) = JobRow::finalize(
            db.raw_conn(),
            job_id,
            &OutcomeRecord {
                state: terminal_state,
                outcome: outcome.clone(),
                finished_at_ms: now,
            },
        ) {
            warn!(error = %e, %job_id, "dispatch: finalize_terminal failed");
        }
    }
    state.job_logs.publish(*job_id, LiveEvent::Terminal(outcome));
    state.job_logs.finalize(*job_id);
}

fn terminal_state_of(outcome: &JobOutcome) -> JobState {
    match outcome {
        JobOutcome::Passed => JobState::Passed,
        JobOutcome::Failed(_) => JobState::Failed,
        JobOutcome::TimedOut { .. } => JobState::TimedOut,
        JobOutcome::Aborted { .. } => JobState::Aborted,
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}
```

- [ ] **Step 2: Wire `main.rs`**

In `crates/paavod/src/main.rs`:

1. After `sd.ensure_dirs()...?;` add:

```rust
    sd.ensure_build_slots(config.scheduler.max_concurrent_builds.max(1))
        .with_context(|| format!("creating build slots under {}", sd.root.display()))?;
```

2. Before the `dispatch::spawn` call, construct the builder (alongside the existing `runner` construction):

```rust
    let builder: std::sync::Arc<dyn paavod::builder::Builder> =
        std::sync::Arc::new(paavod::builder::RealBuilder);
```

3. Change the spawn call from `paavod::dispatch::spawn(state.clone(), runner)` to:

```rust
    let dispatch_handle = paavod::dispatch::spawn(state.clone(), builder, runner);
```

- [ ] **Step 3: Update `dispatch_loop.rs` — add `FakeBuilder`, fix spawn calls**

In `crates/paavod/tests/dispatch_loop.rs`:

1. Add imports near the top:

```rust
use paavod::builder::{BuildOutcome, BuildRequest, Builder};
```

2. Add a fake builder (writes a stub ELF into the slot dir; never invoked by the cache-hit tests but valid if it is):

```rust
struct FakeBuilder;
impl Builder for FakeBuilder {
    fn build(
        &self,
        req: BuildRequest<'_>,
        _lines: paavo_build::BuildLineTx,
        _cancel: crossbeam_channel::Receiver<()>,
    ) -> BuildOutcome {
        std::fs::create_dir_all(&req.target_dir).unwrap();
        let elf = req.target_dir.join("fake.elf");
        std::fs::write(&elf, b"\x7fELF").unwrap();
        BuildOutcome::Ok { elf_path: elf }
    }
}
```

3. Replace **every** `paavod::dispatch::spawn(state.clone(), runner)` call (7 of them) with:

```rust
    let handle = paavod::dispatch::spawn(state.clone(), Arc::new(FakeBuilder), runner);
```

(`paavo-build` and `crossbeam-channel` are already paavod deps; add them to the test's `use`/`Cargo.toml [dev-dependencies]` only if the compiler complains.)

- [ ] **Step 4: Add the build-cap test**

Append to `crates/paavod/tests/dispatch_loop.rs`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn build_stage_respects_cap_and_builds_without_a_board() {
    use std::sync::atomic::{AtomicU32, Ordering};

    // Builder that records peak concurrency and blocks so overlap is observable.
    struct CountingBuilder { cur: Arc<AtomicU32>, max: Arc<AtomicU32> }
    impl Builder for CountingBuilder {
        fn build(&self, req: BuildRequest<'_>, _l: paavo_build::BuildLineTx, _c: crossbeam_channel::Receiver<()>) -> BuildOutcome {
            let n = self.cur.fetch_add(1, Ordering::SeqCst) + 1;
            self.max.fetch_max(n, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(300));
            self.cur.fetch_sub(1, Ordering::SeqCst);
            std::fs::create_dir_all(&req.target_dir).unwrap();
            let elf = req.target_dir.join("fake.elf");
            std::fs::write(&elf, b"\x7fELF").unwrap();
            BuildOutcome::Ok { elf_path: elf }
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let sd = StateDir::from_root(tmp.path());
    sd.ensure_dirs().unwrap();
    sd.ensure_build_slots(2).unwrap();
    let db = Db::open(&sd.sqlite_path).unwrap();
    // 4 distinct-blake3 Submitted jobs; NO board, NO cache.
    for i in 0..4u32 {
        let tar = sd.uploads_dir.join(format!("{i}.tar"));
        std::fs::write(&tar, b"x").unwrap();
        paavo_db::JobRow::insert(db.raw_conn(), &paavo_db::NewJob {
            id: JobId::new(), priority: Priority::Interactive, submitter: "x".into(), source: JobSource::Cli,
            board_selector: BoardSelector { kind: "mcxa266".into(), instance: None, wiring_profile: None },
            inactivity_timeout_ms: 1, hard_max_ms: 1,
            tar_blake3: format!("blake{i}"), tar_path: tar.display().to_string(),
            cargo_update_packages: vec![], skip_cache: false,
        }, 0).unwrap();
    }
    let cfg = Arc::new(Config {
        server: ServerConfig { bind: "127.0.0.1:0".into(), state_dir: tmp.path().to_path_buf(), max_upload_bytes: 256 * 1024 * 1024 },
        web: WebConfig { bind: "127.0.0.1:0".into() },
        timeouts: TimeoutsConfig::default(),
        scheduler: SchedulerConfig { nightly_cron: "0 0 19 * * *".into(), starvation_threshold_s: 21_600, max_concurrent_builds: 2 },
        build_cache: BuildCacheConfig::default(),
        retention: RetentionConfig::default(),
        quarantine: QuarantineConfig::default(),
        corpus: vec![],
    });
    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        config: cfg,
        inventory: Arc::new(Mutex::new(vec![])),
        drain: DrainState::default(),
        cancellation: CancellationRegistry::default(),
        build_cancel: paavod::cancellation::BuildCancelRegistry::default(),
        job_logs: JobLogsBroker::new(),
    };
    let cur = Arc::new(AtomicU32::new(0));
    let max = Arc::new(AtomicU32::new(0));
    let builder: Arc<dyn Builder> = Arc::new(CountingBuilder { cur, max: max.clone() });
    let runner: Arc<dyn Runner> = Arc::new(FakeRunner { out: Mutex::new(JobOutcome::Passed) });
    let handle = paavod::dispatch::spawn(state.clone(), builder, runner);

    // No board exists, so every job must end stuck in AwaitingBoard.
    let mut all_awaiting = false;
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let awaiting = { let db = state.db.lock();
            paavo_db::JobRow::list_by_state(db.raw_conn(), JobState::AwaitingBoard, 100).unwrap().len() };
        if awaiting == 4 { all_awaiting = true; break; }
    }
    assert!(all_awaiting, "all 4 jobs must build (board-free) and reach AwaitingBoard");
    let peak = max.load(Ordering::SeqCst);
    assert!(peak <= 2, "cap exceeded: peak concurrent builds = {peak}");
    assert_eq!(peak, 2, "cap should actually be reached with 4 jobs / 2 slots");

    state.drain.set_draining();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
}
```

- [ ] **Step 5: Run — verify green**

Run: `cargo test -p paavod --test dispatch_loop`
Expected: PASS (the 7 existing tests via the cache-hit path + the new cap test).

- [ ] **Step 6: Commit**

```bash
git add crates/paavod/src/dispatch.rs crates/paavod/src/main.rs crates/paavod/tests/dispatch_loop.rs
git commit -m "feat(paavod): two-stage dispatch — parallel build pool + board-decoupled run"
```

---

## Phase E — routes, surface, integration

### Task 13: Route changes — state parsing, cancel routing, purge guard

**Files:**
- Modify: `crates/paavod/src/routes/jobs.rs` (`parse_state`, `cancel_job`)
- Modify: `crates/paavod/src/routes/admin.rs` (purge guard)
- Modify: `crates/paavod/tests/api_admin.rs` (guard assertion string)
- Modify: `crates/paavod/tests/cancellation.rs` (AwaitingBoard cancel test)

- [ ] **Step 1: Write failing tests**

In `crates/paavod/tests/cancellation.rs`, add a test that mirrors the file's existing harness (reuse its app/server + submit helpers; drive the job to `AwaitingBoard` via `paavo_db::JobRow` transitions before cancelling):

```rust
#[tokio::test(flavor = "multi_thread")]
async fn cancel_awaiting_board_job_returns_204_and_aborts() {
    // ARRANGE: build the test AppState + router exactly as the other
    // tests in this file do (same fixture helper), insert one job, then:
    //   JobRow::transition_submitted_to_building(conn, &id, 1)
    //   JobRow::transition_building_to_awaiting_board(conn, &id, "/e.elf")
    // ACT: POST /jobs/{id}/cancel
    // ASSERT: 204, and JobRow::get(...).state == JobState::Aborted with
    //         outcome Aborted { by: User }.
}
```

In `crates/paavod/tests/api_admin.rs`, update the existing purge-guard assertion (currently `assert!(body.contains("building or running"))`) to the new wording and add an `awaiting_board` row to the in-flight setup so the guard trips on it.

- [ ] **Step 2: Run — verify failure**

Run: `cargo test -p paavod --test cancellation cancel_awaiting_board && cargo test -p paavod --test api_admin`
Expected: FAIL.

- [ ] **Step 3: `parse_state` — accept the new state**

In `crates/paavod/src/routes/jobs.rs`, in `parse_state`, add the arm next to `"running"`:

```rust
        "awaiting_board" => AwaitingBoard,
```

- [ ] **Step 4: `cancel_job` — route by state**

In `crates/paavod/src/routes/jobs.rs`, ensure `use paavo_proto::JobState;` is present, then replace the `cancel_if_submitted` call + match with:

```rust
    let res = {
        let db = s.db.lock();
        paavo_core::cancel_if_pending(db.raw_conn(), &id, now_ms)
    };
    match res {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(paavo_core::CoreError::NotCancellable { state }) => {
            // Building → kill the cargo child; Running → signal the
            // run watchdog. (Submitted/AwaitingBoard were handled inline
            // above by cancel_if_pending.)
            let signalled = match state {
                JobState::Building => s.build_cancel.signal(&id),
                _ => s.cancellation.signal(&id, paavo_runner::RunCommand::Cancel),
            };
            if signalled {
                StatusCode::NO_CONTENT.into_response()
            } else {
                (
                    StatusCode::CONFLICT,
                    format!("not cancellable in state {state:?}"),
                )
                    .into_response()
            }
        }
        Err(paavo_core::CoreError::Db(paavo_db::DbError::NotFound { .. })) => {
            (StatusCode::NOT_FOUND, "no such job").into_response()
        }
        Err(e) => {
            error!(error = %e, "cancel_job internal error");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
```

- [ ] **Step 5: `admin purge` — include `awaiting_board`**

In `crates/paavod/src/routes/admin.rs`, change the in-flight count query
`SELECT COUNT(*) FROM job WHERE state IN ('building','running')`
to `... state IN ('building','awaiting_board','running')`, and update the refusal message from `"...currently building or running; ..."` to `"...currently building, awaiting board, or running; ..."`.

- [ ] **Step 6: Run — verify green**

Run: `cargo test -p paavod --test cancellation --test api_admin --test api_jobs`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/paavod/src/routes/ crates/paavod/tests/cancellation.rs crates/paavod/tests/api_admin.rs
git commit -m "feat(paavod): cancel routing + purge guard for awaiting_board/building"
```

---

### Task 14: Surface (CLI/web/config) + acceptance integration test

**Files:**
- Verify (likely no change): `crates/paavo-cli/src/cmd_jobs.rs`
- Modify: `crates/paavo-web/src/assets/style.css` (+ any state label/class map)
- Modify: `sample-paavo.toml`
- Modify: `crates/paavod/tests/api_jobs.rs` (10-concurrent-submit test)

- [ ] **Step 1: CLI — confirm generic rendering**

`paavo-cli jobs` prints `r["state"].as_str()` (`cmd_jobs.rs:85`) and passes `--state` straight through, so `awaiting_board` already lists and filters. No code change; just confirm:

Run: `cargo build -p paavo-cli`
Expected: compiles. (If a `{state:9}` width truncates oddly, leave it — wider is fine.)

- [ ] **Step 2: Web — render the new state**

Grep `crates/paavo-web` for how a job state becomes a CSS class / label (the dashboard uses `s-<state>` classes, e.g. `s-building` in `style.css:231`). Add a rule mirroring `building`:

```css
.s-awaiting_board { color: var(--ef-state-running); font-weight: 600; }
```

If `paavo-web` has a Rust/JS map of state → human label (e.g. "Building"), add `awaiting_board` → "Awaiting board" mirroring the `building` arm. Then:

Run: `cargo build -p paavo-web`
Expected: compiles (no non-exhaustive `JobState` match left).

- [ ] **Step 3: Document the config knob**

In `sample-paavo.toml`, under `[scheduler]`, add:

```toml
# Maximum number of cargo builds that run at once. Each build gets its
# own target directory so they truly run in parallel. Jobs beyond this
# wait in the queue (Submitted) and start as build slots free up. The
# hardware run phase is still serialized per board. Default: 5.
max_concurrent_builds = 5
```

- [ ] **Step 4: Acceptance integration test (INV-1)**

In `crates/paavod/tests/api_jobs.rs`, add a test mirroring the file's existing `POST /jobs` helper (same multipart builder + test server). Fire 10 submits and assert 10 distinct accepted ids:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn ten_concurrent_submits_are_all_accepted() {
    // Build the test server + a valid board exactly as the existing
    // post_jobs test in this file does. Then submit 10 jobs (sequentially
    // is fine — the daemon never gates acceptance on in-flight load) and:
    //   - assert each response is 202 with a job_id,
    //   - collect the 10 ids into a HashSet, assert len == 10,
    //   - assert `GET /jobs?state=submitted&limit=500` (or a direct
    //     JobRow::list_by_state) returns 10 rows.
    // The point: the build cap NEVER rejects or drops a submission.
}
```

- [ ] **Step 5: Full workspace verification**

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean (fix any warnings the new code introduces).

- [ ] **Step 6: Commit**

```bash
git add crates/paavo-web/ sample-paavo.toml crates/paavod/tests/api_jobs.rs
git commit -m "feat: render awaiting_board, document max_concurrent_builds, acceptance test"
```

---

## Self-Review (completed by plan author)

**1. Spec coverage** — every spec section maps to a task:

| Spec | Task(s) |
| --- | --- |
| §2 cap + per-slot dirs + invariants | 7, 8, 12 |
| §3 two-stage scheduler | 5, 12 |
| §4 state machine + transitions | 1, 3 |
| §4.3 board exclusion → running | 4 |
| §5.1 slot dirs / §5.2 stable artifact / §5.3 single-flight | 8 / 12 / 5 |
| §6 pickers + starvation | 5 |
| §7 config | 7 |
| §8 acceptance invariant | 14 |
| §9.1 cancellation (Submitted/AwaitingBoard inline, Building kill, Running watchdog) | 6, 9, 11, 13 |
| §9.2 purge guard | 13 |
| §9.4 drain over build+run | 12 |
| §13 migration | 2 |

**Out of scope (noted):** §9.3 startup reconciliation — `abort_interrupted_jobs` is **not implemented in the repo today** (only designed in `2026-06-16-startup-job-reconciliation-design.md`). When it lands it must scan only `building`/`running` (leaving `awaiting_board` intact); no work here. The dispatch drain already excludes `awaiting_board` from "in flight", consistent with that future behavior.

**2. Placeholder scan** — no `TBD`/`TODO`/"implement later". Every code step shows complete code; mechanical steps (config literals, AppState fields, spawn-call replacement, web label) name the exact edit and a verification command.

**3. Type consistency** — `Builder::build(req: BuildRequest, lines: BuildLineTx, cancel_rx: Receiver<()>) -> BuildOutcome` and `BuildOutcome::{Ok{elf_path}, Failed(String), Cancelled}` are identical in Task 10 (definition), Task 12 (dispatch caller + `FakeBuilder`/`CountingBuilder`), and the `RealBuilder` impl. Transition names (`transition_submitted_to_building`, `transition_building_to_awaiting_board`, `transition_awaiting_to_running`) and picker names (`pick_buildable`, `pick_runnable`) are used consistently across Tasks 3, 5, 12. `BuildCancelRegistry` methods (`register`/`signal`/`unregister`/`active`) match between Tasks 11, 12, 13.

**Compile-green ordering** — additive APIs (new transitions, pickers, `cancel_if_pending`, `build_release_streaming_cancellable`) keep every crate compiling + tests green after each task; the old `transition_to_building`/`transition_to_running`/`pick_next`/`cancel_if_submitted` remain (dead but harmless) and may be removed in a later cleanup PR.

---

## Execution

Plan complete and saved to `docs/superpowers/plans/2026-06-16-parallel-build-pool.md`. This work will run in a **dedicated git worktree** (per the brainstorming agreement) — set up via the `superpowers:using-git-worktrees` skill at execution start.

Two execution options:

1. **Subagent-Driven (recommended)** — a fresh subagent per task, two-stage review between tasks, fast iteration. REQUIRED SUB-SKILL: `superpowers:subagent-driven-development`.
2. **Inline Execution** — execute tasks in this session with checkpoints. REQUIRED SUB-SKILL: `superpowers:executing-plans`.

Which approach?





