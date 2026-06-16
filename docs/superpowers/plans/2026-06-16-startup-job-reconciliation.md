# Startup Job Reconciliation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On paavod startup, terminalize orphaned `building`/`running` jobs as `Aborted { by: Interrupted }` with a forensic log line, so a daemon crash mid-job no longer deadlocks `admin purge` or leaves a job stuck "running".

**Architecture:** A new `AbortReason::Interrupted` wire variant; a new `JobRow::abort_interrupted_jobs(conn, now_ms) -> u64` that sweeps all in-flight rows in one transaction (forensic frame + state update per orphan); and a three-line call in `main.rs` after `Db::open` and before `dispatch::spawn`. No schema migration.

**Tech Stack:** Rust (rusqlite, serde, chrono), SQLite.

**Spec:** `docs/superpowers/specs/2026-06-16-startup-job-reconciliation-design.md`

---

## Task 1: `AbortReason::Interrupted` + wire tests (TDD)

**Files:**
- Modify: `crates/paavo-proto/src/job.rs`
- Modify: `crates/paavo-proto/tests/serde_roundtrip.rs`
- Modify: `crates/paavo-proto/tests/wire_compat.rs`

- [ ] **Step 1: Write the failing wire tests**

In `crates/paavo-proto/tests/wire_compat.rs`, add a test mirroring the existing `terminal_aborted_daemon_shutdown_matches_historical` (find it for the surrounding imports/helpers — `assert_roundtrip`, `WireMessage`, `JobOutcome`, `AbortReason` are already in scope there):

```rust
#[test]
fn terminal_aborted_interrupted_matches_historical() {
    let expected = r#"{"type":"terminal","outcome":{"aborted":{"by":"interrupted"}}}"#;
    assert_roundtrip(
        WireMessage::Terminal {
            outcome: JobOutcome::Aborted {
                by: AbortReason::Interrupted,
            },
        },
        expected,
    );
}
```

In `crates/paavo-proto/tests/serde_roundtrip.rs`, find the `outcomes` vec that already contains the two `JobOutcome::Aborted { by: ... }` entries (User and DaemonShutdown) and add a third entry right after the DaemonShutdown one:

```rust
        JobOutcome::Aborted {
            by: paavo_proto::AbortReason::Interrupted,
        },
```

- [ ] **Step 2: Run the tests to verify they fail (compile error)**

Run: `cargo test -p paavo-proto`
Expected: FAIL to compile — `AbortReason::Interrupted` does not exist yet. This is the red.

- [ ] **Step 3: Add the variant**

In `crates/paavo-proto/src/job.rs`, find:

```rust
pub enum AbortReason {
    /// `paavo-cli cancel`.
    User,
    /// SIGTERM drain ran out of grace.
    DaemonShutdown,
}
```

Replace with:

```rust
pub enum AbortReason {
    /// `paavo-cli cancel`.
    User,
    /// SIGTERM drain ran out of grace.
    DaemonShutdown,
    /// paavod restarted while this job was still building/running; the
    /// startup reconciliation pass terminalized the orphaned row.
    Interrupted,
}
```

The enum already carries `#[serde(rename_all = "snake_case")]`, so the new variant serializes to `"interrupted"` with no further annotation.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p paavo-proto`
Expected: PASS — the new `terminal_aborted_interrupted_matches_historical` test plus the extended `serde_roundtrip`, and all pre-existing proto tests.

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-proto/src/job.rs crates/paavo-proto/tests/serde_roundtrip.rs crates/paavo-proto/tests/wire_compat.rs
git commit -m "feat(paavo-proto): add AbortReason::Interrupted"
```

(Keep the commit-message subject and body wrapped at 72 columns.)

---

## Task 2: `JobRow::abort_interrupted_jobs` + test (TDD)

**Files:**
- Modify: `crates/paavo-db/src/job.rs`
- Modify: `crates/paavo-db/tests/job_ops.rs`

- [ ] **Step 1: Write the failing test**

In `crates/paavo-db/tests/job_ops.rs`, add this test. It uses the existing `fresh_db()` helper plus `insert_default_board()` (which seeds a board with id `"mcxa266-01"` — `transition_to_building` needs it for the `job.board_id` FK) and `sample_new_job(id)` (inserts a job in `submitted`).

```rust
#[test]
fn abort_interrupted_jobs_terminalizes_in_flight_only() {
    use paavo_db::LogFrameDb;
    use paavo_proto::{AbortReason, JobOutcome, JobState, LogFrame, LogLevel};

    let db = fresh_db();
    insert_default_board(&db);
    let conn = db.raw_conn();

    // submitted: untouched (not orphaned).
    let submitted = JobId::new();
    JobRow::insert(conn, &sample_new_job(submitted), 0).unwrap();

    // building: orphaned -> aborted.
    let building = JobId::new();
    JobRow::insert(conn, &sample_new_job(building), 0).unwrap();
    JobRow::transition_to_building(conn, &building, "mcxa266-01", 1000).unwrap();

    // running: orphaned -> aborted, with two pre-existing log frames.
    let running = JobId::new();
    JobRow::insert(conn, &sample_new_job(running), 0).unwrap();
    JobRow::transition_to_building(conn, &running, "mcxa266-01", 1000).unwrap();
    JobRow::transition_to_running(conn, &running, "/tmp/x.elf").unwrap();
    let pre = vec![
        LogFrame { seq: 0, ts_us: 10, level: LogLevel::Info, target: Some("cargo:stdout".into()), message: "l0".into() },
        LogFrame { seq: 1, ts_us: 20, level: LogLevel::Info, target: Some("cargo:stdout".into()), message: "l1".into() },
    ];
    LogFrame::append_batch(conn, &running, &pre).unwrap();

    // passed: terminal, untouched.
    let passed = JobId::new();
    JobRow::insert(conn, &sample_new_job(passed), 0).unwrap();
    JobRow::transition_to_building(conn, &passed, "mcxa266-01", 1000).unwrap();
    JobRow::transition_to_running(conn, &passed, "/tmp/y.elf").unwrap();
    JobRow::finalize(
        conn,
        &passed,
        &paavo_db::OutcomeRecord {
            state: JobState::Passed,
            outcome: JobOutcome::Passed,
            finished_at_ms: 2000,
        },
    )
    .unwrap();

    // Reconcile at now_ms = 5000.
    let n = JobRow::abort_interrupted_jobs(conn, 5000).unwrap();
    assert_eq!(n, 2, "exactly the building + running jobs are reconciled");

    // building + running are now aborted/interrupted.
    for id in [&building, &running] {
        let row = JobRow::get(conn, id).unwrap();
        assert_eq!(row.state, JobState::Aborted, "in-flight job aborted");
        assert_eq!(
            row.outcome,
            Some(JobOutcome::Aborted { by: AbortReason::Interrupted }),
        );
    }

    // Forensic frame: running job's lands at seq 2 (after 0,1), warn level.
    let frames = LogFrame::list(conn, &running, 0, 100).unwrap();
    assert_eq!(frames.len(), 3, "two original + one forensic");
    let forensic = &frames[2];
    assert_eq!(forensic.seq, 2);
    assert_eq!(forensic.level, LogLevel::Warn);
    assert_eq!(forensic.ts_us, (5000 - 1000) * 1000, "ts_us continues timeline from started_at");
    assert!(forensic.message.contains("interrupted"), "forensic msg: {}", forensic.message);

    // building job (no prior frames) gets its forensic frame at seq 0.
    let bframes = LogFrame::list(conn, &building, 0, 100).unwrap();
    assert_eq!(bframes.len(), 1);
    assert_eq!(bframes[0].seq, 0);
    assert_eq!(bframes[0].level, LogLevel::Warn);

    // submitted + passed untouched.
    assert_eq!(JobRow::get(conn, &submitted).unwrap().state, JobState::Submitted);
    assert_eq!(JobRow::get(conn, &passed).unwrap().state, JobState::Passed);
    assert_eq!(LogFrame::list(conn, &submitted, 0, 100).unwrap().len(), 0);

    // Idempotent: a second call finds nothing.
    assert_eq!(JobRow::abort_interrupted_jobs(conn, 6000).unwrap(), 0);
    assert_eq!(LogFrame::list(conn, &running, 0, 100).unwrap().len(), 3, "no new frames on re-run");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p paavo-db --test job_ops abort_interrupted_jobs_terminalizes_in_flight_only`
Expected: FAIL to compile — `JobRow::abort_interrupted_jobs` does not exist yet.

- [ ] **Step 3: Add `AbortReason` to the imports**

In `crates/paavo-db/src/job.rs`, change the proto import line:

```rust
use paavo_proto::{BoardSelector, JobId, JobOutcome, JobSource, JobState, Priority};
```

to:

```rust
use paavo_proto::{AbortReason, BoardSelector, JobId, JobOutcome, JobSource, JobState, Priority};
```

- [ ] **Step 4: Implement the method**

In `crates/paavo-db/src/job.rs`, add this method inside `impl JobRow { ... }`, right after `finalize` (before the closing `}` of the impl block):

```rust
    /// Terminalize every job still in `building`/`running` as
    /// `Aborted { by: Interrupted }`, appending a forensic `Warn`
    /// log frame to each. Returns the count reconciled.
    ///
    /// Intended to run ONCE at daemon startup, before the dispatch
    /// loop: any in-flight row at that point is provably orphaned (a
    /// fresh process has no worker thread for it; single-writer
    /// assumption, spec §7). The whole sweep runs in one transaction,
    /// so a crash mid-sweep commits nothing and a re-run is clean;
    /// the operation is idempotent (a second call finds nothing and
    /// returns 0).
    pub fn abort_interrupted_jobs(conn: &Connection, now_ms: i64) -> Result<u64> {
        let outcome_json = serde_json::to_string(&JobOutcome::Aborted {
            by: AbortReason::Interrupted,
        })?;
        const MSG: &str = "job interrupted: paavod restarted while this \
             job was in-flight; any output above is partial";

        let tx = conn.unchecked_transaction()?;
        // Snapshot the orphaned rows (id + started_at). Collected into a
        // Vec so the prepared statement is dropped before the writes.
        let orphans: Vec<(String, Option<i64>)> = {
            let mut stmt = tx.prepare(
                "SELECT id, started_at FROM job
                 WHERE state IN ('building','running')",
            )?;
            stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };

        for (id, started_at) in &orphans {
            // `started_at` is `&Option<i64>`; copy it out (Option<i64>
            // is Copy) so we can `.map` it by value below.
            let started_at = *started_at;
            // Next seq after whatever was captured before the crash.
            let next_seq: i64 = tx.query_row(
                "SELECT COALESCE(MAX(seq), -1) + 1 FROM log_frame
                 WHERE job_id = ?1",
                params![id],
                |r| r.get(0),
            )?;
            // Continue the job's relative timeline from started_at.
            let ts_us: i64 = started_at
                .map(|s| (now_ms - s).max(0).saturating_mul(1000))
                .unwrap_or(0);
            tx.execute(
                "INSERT INTO log_frame
                   (job_id, seq, ts_us, level, target, message)
                 VALUES (?1, ?2, ?3, 'warn', NULL, ?4)",
                params![id, next_seq, ts_us, MSG],
            )?;
            tx.execute(
                "UPDATE job
                 SET state = 'aborted', outcome_detail = ?1, finished_at = ?2
                 WHERE id = ?3",
                params![outcome_json, now_ms, id],
            )?;
        }
        tx.commit()?;
        Ok(orphans.len() as u64)
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p paavo-db --test job_ops abort_interrupted_jobs_terminalizes_in_flight_only`
Expected: PASS.

- [ ] **Step 6: Run the full paavo-db suite + clippy + fmt**

Run: `cargo test -p paavo-db`
Run: `cargo clippy -p paavo-db --all-targets -- -D warnings`
Run: `cargo fmt --all -- --check`
Expected: all clean. (If fmt reports a diff, run `cargo fmt --all` and re-check.)

- [ ] **Step 7: Commit**

```bash
git add crates/paavo-db/src/job.rs crates/paavo-db/tests/job_ops.rs
git commit -m "feat(paavo-db): JobRow::abort_interrupted_jobs startup sweep"
```

---

## Task 3: Wire into `main.rs` startup + final gate

**Files:**
- Modify: `crates/paavod/src/main.rs`

- [ ] **Step 1: Add the reconciliation call**

In `crates/paavod/src/main.rs`, find the line that opens the DB:

```rust
    let db = paavo_db::Db::open(&sd.sqlite_path)
        .with_context(|| format!("opening sqlite at {}", sd.sqlite_path.display()))?;
```

Immediately after it (and before `let inventory = load_inventory(...)`), insert:

```rust

    // Self-heal: terminalize any job left `building`/`running` by a
    // previous paavod that died mid-job. A fresh process has no worker
    // for those rows, so they are provably orphaned. Must run before
    // `dispatch::spawn` so the loop never sees stale in-flight rows,
    // and it un-deadlocks `admin purge` (which refuses while any job
    // is in-flight). See the startup-reconciliation design spec.
    let reconciled = paavo_db::JobRow::abort_interrupted_jobs(
        db.raw_conn(),
        chrono::Utc::now().timestamp_millis(),
    )
    .context("reconciling orphaned in-flight jobs at startup")?;
    if reconciled > 0 {
        tracing::warn!(
            reconciled,
            "startup: aborted orphaned in-flight jobs (interrupted)"
        );
    }
```

(`chrono` is already used fully-qualified elsewhere in `main.rs`; `.context(...)` comes from the `anyhow::Context` trait already imported there. If the compiler reports `Context` is not in scope, add `use anyhow::Context;` — but it is almost certainly already imported, since other `?` sites in `main.rs` use `.with_context`.)

- [ ] **Step 2: Build + full workspace gate**

Run: `cargo build -p paavod`
Run: `cargo fmt --all -- --check`  (run `cargo fmt --all` first if needed)
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Run: `cargo test --workspace`
Expected: all green.

- [ ] **Step 3: Commit**

```bash
git add crates/paavod/src/main.rs
git commit -m "feat(paavod): reconcile orphaned jobs on startup"
```

---

## Notes for the executor

- **Task order:** Task 1 (proto variant) must land before Task 2 (which constructs `AbortReason::Interrupted`) and Task 3.
- **`ts_us` math:** `(now_ms - started_at).max(0).saturating_mul(1000)` stays non-negative and can't overflow; `started_at` is non-NULL for building/running rows in practice (set by `transition_to_building`), but the `unwrap_or(0)` fallback is there for safety.
- **Commit messages:** wrap every line (subject and body) at 72 columns.
- **No migration, no `main.rs` unit test:** the call site is tested transitively (the method has its own test); this matches how the rest of `main.rs` startup wiring is treated.
