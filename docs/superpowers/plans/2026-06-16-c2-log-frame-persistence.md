# C2 Log-Frame Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist build-phase and run-phase log frames to `log_frame` so a viewer who opens `/jobs/:id` after a job finished (or whose `EventSource` reconnects) sees the full log, with monotonic seqs and a single timeline.

**Architecture:** A shared per-job seq counter (`Arc<AtomicU64>`) and job-start `Instant` are created in `dispatch::run_one_inner` and reach both forwarders — the build forwarder via plain params, the run forwarder via a new `RunContext` threaded through paavo-core's `Runner` trait. Both forwarders feed a unit-testable `FrameSink` (new `crates/paavod/src/log_sink.rs`) that assigns seq, stamps `ts_us`, publishes to the live broker, and batch-persists to `log_frame`. The browser dedups by `lastSeq` (correctness) while the SSE proxy trims the SSR-covered prefix via `?since_seq=N` (bandwidth).

**Tech Stack:** Rust (axum, rusqlite, crossbeam-channel, parking_lot, tokio broadcast), vanilla JS (EventSource), SQLite.

**Spec:** `docs/superpowers/specs/2026-06-16-c2-log-frame-persistence-design.md`

---

## Task 1: `RunContext` + `Runner` trait change (compile-green refactor)

Pure seam change. No behavior change. The guard is "the workspace still compiles and all existing tests pass." Five `Runner` implementors must all move to the new signature in this one task so the build never breaks.

**Files:**
- Modify: `crates/paavo-core/src/runner.rs`
- Modify: `crates/paavo-core/src/lib.rs` (re-export `RunContext`)
- Modify: `crates/paavod/src/real_runner.rs` (impl)
- Modify: `crates/paavod/src/main.rs` (dev `FakeRunner` impl)
- Modify: `crates/paavod/src/dispatch.rs` (`run_one_inner` constructs `RunContext`)
- Modify: `crates/paavod/tests/dispatch_loop.rs` (three test doubles)

- [ ] **Step 1: Define `RunContext` and change the trait**

In `crates/paavo-core/src/runner.rs`, replace the trait with the version below and add the struct. Keep `RunOutcome` as-is.

```rust
//! Abstraction over `paavo-runner::run_job`. Production code wires this to
//! the real BoardWorker. Tests substitute a deterministic in-process impl.

use paavo_proto::JobOutcome;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

/// What a runner reports back when a job finishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// Terminal job outcome.
    pub outcome: JobOutcome,
    /// True if the BoardWorker successfully released the probe before the
    /// release-grace expired. Per spec §5.2, when this is `false` and the
    /// outcome is `TimedOut{Inactivity}`, the board's infra-failure counter
    /// must be bumped.
    pub probe_released_cleanly: bool,
}

/// Per-job context handed to `Runner::run`. Carries the job/board ids plus
/// the shared log-frame seq counter and job-start clock so the run-phase
/// log forwarder numbers and timestamps its frames continuously with the
/// build-phase forwarder (both restamp from this shared state). See
/// `docs/superpowers/specs/2026-06-16-c2-log-frame-persistence-design.md`.
pub struct RunContext<'a> {
    /// Job being run.
    pub job_id: paavo_proto::JobId,
    /// Board the job was dispatched to.
    pub board_id: &'a str,
    /// Shared per-job log-frame seq counter (created in dispatch, also
    /// handed to the build forwarder). `fetch_add(1, Relaxed)` per frame.
    pub log_seq: Arc<AtomicU64>,
    /// Monotonic job-execution start. `ts_us` is microseconds since this.
    pub job_start: Instant,
}

/// Production code passes `Arc<dyn Runner>`; tests pass `FakeRunner`.
pub trait Runner: Send + Sync {
    /// Run a job on `ctx.board_id` and block until terminal. The job has
    /// already had its row transitioned to `Building` and its tar/ELF
    /// resolved by the caller.
    fn run(&self, ctx: RunContext<'_>) -> RunOutcome;
}
```

- [ ] **Step 2: Re-export `RunContext` from paavo-core**

In `crates/paavo-core/src/lib.rs`, find the existing `pub use runner::{...}` line and add `RunContext`:

```rust
pub use runner::{RunContext, RunOutcome, Runner};
```

(If the existing line reads `pub use runner::{RunOutcome, Runner};`, change it to the above. If `runner`'s items are re-exported individually, add `pub use runner::RunContext;`.)

- [ ] **Step 3: Update `RealRunner::run` to the new signature**

In `crates/paavod/src/real_runner.rs`, change the impl header. The body uses `job_id` and `board_id` exactly as before; destructure with `..` so the not-yet-used `log_seq`/`job_start` don't warn (Task 4 binds them).

Find:

```rust
impl Runner for RealRunner {
    fn run(&self, job_id: JobId, board_id: &str) -> RunOutcome {
```

Replace with:

```rust
impl Runner for RealRunner {
    fn run(&self, ctx: paavo_core::RunContext<'_>) -> RunOutcome {
        let paavo_core::RunContext { job_id, board_id, .. } = ctx;
```

Leave the rest of the function body unchanged.

- [ ] **Step 4: Update the dev `FakeRunner` in `main.rs`**

In `crates/paavod/src/main.rs`, find the `impl paavo_core::Runner for FakeRunner` block:

```rust
    fn run(&self, _job_id: JobId, _board_id: &str) -> paavo_core::RunOutcome {
```

Replace with:

```rust
    fn run(&self, _ctx: paavo_core::RunContext<'_>) -> paavo_core::RunOutcome {
```

If `JobId` is now an unused import in `main.rs` after this change, remove it from the `use` line (the compiler will flag it).

- [ ] **Step 5: Construct `RunContext` in `dispatch::run_one_inner`**

In `crates/paavod/src/dispatch.rs`, at the very top of `run_one_inner` (right after `let job_id = job.id;`), create the shared state:

```rust
    let job_id = job.id;

    // Shared per-job log-frame seq counter + monotonic job-start clock.
    // Both reach the build forwarder (via build_or_cache, Task 3) and the
    // run forwarder (via RunContext, below) so build- and run-phase frames
    // share one contiguous seq space and one timeline. See the C2 spec.
    let job_start = std::time::Instant::now();
    let log_seq = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
```

Then find the run call near the end of the function:

```rust
    let RunOutcome {
        outcome,
        probe_released_cleanly,
    } = runner.run(job_id, board_id);
```

Replace with:

```rust
    let RunOutcome {
        outcome,
        probe_released_cleanly,
    } = runner.run(paavo_core::RunContext {
        job_id,
        board_id,
        log_seq: log_seq.clone(),
        job_start,
    });
```

(`log_seq.clone()` keeps a handle for Task 3's `build_or_cache` call; in this task `log_seq`/`job_start` are otherwise unused — that is fine, no warning, because they are moved/cloned into `RunContext`.)

- [ ] **Step 6: Update the three test doubles in `dispatch_loop.rs`**

In `crates/paavod/tests/dispatch_loop.rs`, update all three `Runner` impls. For `FakeRunner` (around line 22):

```rust
impl Runner for FakeRunner {
    fn run(&self, _ctx: paavo_core::RunContext<'_>) -> RunOutcome {
        RunOutcome {
            outcome: self.out.lock().clone(),
            probe_released_cleanly: true,
        }
    }
}
```

For `PanickyRunner` (around line 356):

```rust
    impl Runner for PanickyRunner {
        fn run(&self, _ctx: paavo_core::RunContext<'_>) -> RunOutcome {
```

For `CountingRunner` (around line 403):

```rust
    impl Runner for CountingRunner {
        fn run(&self, _ctx: paavo_core::RunContext<'_>) -> RunOutcome {
```

Leave each body unchanged. Add `use paavo_core::RunContext;` is NOT needed since we qualify as `paavo_core::RunContext`; the existing `use paavo_core::{RunOutcome, Runner};` stays.

- [ ] **Step 7: Build and run the full test suite to verify the refactor is green**

Run: `cargo test --workspace`
Expected: PASS — same test counts as the baseline (this is a pure signature refactor). If anything fails to compile, it is a missed implementor or call site; fix it before committing.

- [ ] **Step 8: Commit**

```bash
git add crates/paavo-core/src/runner.rs crates/paavo-core/src/lib.rs crates/paavod/src/real_runner.rs crates/paavod/src/main.rs crates/paavod/src/dispatch.rs crates/paavod/tests/dispatch_loop.rs
git commit -m "refactor(paavo-core): Runner::run takes RunContext (seq counter + job clock)"
```

---

## Task 2: `FrameSink` with unit tests (TDD)

The testable core both forwarders will feed. Build it test-first.

**Files:**
- Create: `crates/paavod/src/log_sink.rs`
- Modify: `crates/paavod/src/lib.rs` (declare module)

- [ ] **Step 1: Declare the module**

In `crates/paavod/src/lib.rs`, add to the `pub mod` list (alphabetical-ish, near `job_logs`):

```rust
pub mod log_sink;
```

- [ ] **Step 2: Write `FrameSink` with method stubs + the failing tests**

Create `crates/paavod/src/log_sink.rs` with the full content below. The methods are real (not stubs) but we run the tests next to confirm they exercise the behavior; if you prefer strict red-green, temporarily replace the `push`/`flush` bodies with `todo!()`, watch them fail, then restore. The content as written is the final implementation + tests.

```rust
//! `FrameSink`: the shared persistence core for both log forwarders.
//!
//! The build forwarder (dispatch) and the run forwarder (real_runner) are
//! sequential phases of one job. Each constructs a `FrameSink` from the
//! shared `Arc<AtomicU64>` seq counter + the shared job-start `Instant`,
//! then feeds it frames. The sink assigns the authoritative seq, stamps
//! `ts_us` against the shared clock, publishes to the live broker, and
//! batch-persists to `log_frame`. Because the phases are strictly
//! sequential (the build forwarder is joined before the run forwarder
//! spawns), the shared counter is never accessed concurrently and the
//! `(job_id, seq)` rows are contiguous across the build→run boundary.
//!
//! See `docs/superpowers/specs/2026-06-16-c2-log-frame-persistence-design.md`.

use crate::job_logs::{JobLogsBroker, LiveEvent};
use paavo_db::{Db, LogFrameDb};
use paavo_proto::{JobId, LogFrame, LogLevel};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Flush after this many buffered frames.
const BATCH_MAX: usize = 64;
/// Flush at least this often while frames are arriving.
const FLUSH_INTERVAL: Duration = Duration::from_millis(50);

/// Per-forwarder log sink: assign seq, stamp ts_us, publish, batch-persist.
pub struct FrameSink {
    job_id: JobId,
    broker: JobLogsBroker,
    db: Arc<Mutex<Db>>,
    seq: Arc<AtomicU64>,
    job_start: Instant,
    batch: Vec<LogFrame>,
    last_flush: Instant,
}

impl FrameSink {
    /// Construct a sink. `seq` and `job_start` are shared across the
    /// build and run forwarders so frames stay contiguous + monotonic.
    pub fn new(
        job_id: JobId,
        broker: JobLogsBroker,
        db: Arc<Mutex<Db>>,
        seq: Arc<AtomicU64>,
        job_start: Instant,
    ) -> Self {
        Self {
            job_id,
            broker,
            db,
            seq,
            job_start,
            batch: Vec::with_capacity(BATCH_MAX),
            last_flush: Instant::now(),
        }
    }

    /// Ingest one frame: assign seq + ts_us, publish live, buffer, and
    /// flush if the batch is full or the flush interval elapsed.
    pub fn push(&mut self, level: LogLevel, target: Option<String>, message: String) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let ts_us = u64::try_from(self.job_start.elapsed().as_micros()).unwrap_or(u64::MAX);
        let frame = LogFrame {
            seq,
            ts_us,
            level,
            target,
            message,
        };
        self.broker
            .publish(self.job_id, LiveEvent::Frame(frame.clone()));
        self.batch.push(frame);
        if self.batch.len() >= BATCH_MAX || self.last_flush.elapsed() >= FLUSH_INTERVAL {
            self.flush();
        }
    }

    /// Called by a forwarder on its `recv_timeout` timeout: flush a
    /// non-empty batch if the interval elapsed, so frames don't sit
    /// unpersisted while the source is quiet.
    pub fn tick(&mut self) {
        if !self.batch.is_empty() && self.last_flush.elapsed() >= FLUSH_INTERVAL {
            self.flush();
        }
    }

    /// Final flush; call once when the source channel closes.
    pub fn finish(mut self) {
        if !self.batch.is_empty() {
            self.flush();
        }
    }

    fn flush(&mut self) {
        {
            let conn = self.db.lock();
            if let Err(e) = LogFrame::append_batch(conn.raw_conn(), &self.job_id, &self.batch) {
                // A DB write failure leaves a gap in the historical view
                // but MUST NOT abort the build or run; the live broker
                // already delivered every frame.
                tracing::error!(
                    error = %e,
                    job_id = %self.job_id,
                    "log forwarder: append_batch failed; frames lost from history"
                );
            }
        }
        self.batch.clear();
        self.last_flush = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paavo_proto::{
        BoardSelector, JobSource, LogFrame as ProtoFrame, Priority,
    };
    use tempfile::TempDir;

    fn temp_db() -> (TempDir, Arc<Mutex<Db>>) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.sqlite")).unwrap();
        (dir, Arc::new(Mutex::new(db)))
    }

    /// log_frame.job_id is `REFERENCES job(id)` with foreign_keys = ON,
    /// so a job row must exist before append_batch.
    fn seed_job(db: &Arc<Mutex<Db>>) -> JobId {
        let id = JobId::new();
        let conn = db.lock();
        paavo_db::JobRow::insert(
            conn.raw_conn(),
            &paavo_db::NewJob {
                id,
                priority: Priority::Interactive,
                submitter: "test".into(),
                source: JobSource::Cli,
                board_selector: BoardSelector {
                    kind: "mcxa266".into(),
                    instance: None,
                    wiring_profile: None,
                },
                inactivity_timeout_ms: 120_000,
                hard_max_ms: 900_000,
                tar_blake3: "x".into(),
                tar_path: "/tmp/x.tar".into(),
                cargo_update_packages: vec![],
                skip_cache: false,
            },
            0,
        )
        .unwrap();
        id
    }

    fn rows(db: &Arc<Mutex<Db>>, id: &JobId) -> Vec<ProtoFrame> {
        let conn = db.lock();
        ProtoFrame::list(conn.raw_conn(), id, 0, 10_000).unwrap()
    }

    #[test]
    fn push_assigns_monotonic_seq_and_persists() {
        let (_d, db) = temp_db();
        let id = seed_job(&db);
        let seq = Arc::new(AtomicU64::new(0));
        let mut sink = FrameSink::new(id, JobLogsBroker::new(), db.clone(), seq, Instant::now());
        sink.push(LogLevel::Info, Some("cargo:stdout".into()), "a".into());
        sink.push(LogLevel::Warn, None, "b".into());
        sink.push(LogLevel::Error, None, "c".into());
        sink.finish();

        let got = rows(&db, &id);
        assert_eq!(got.len(), 3, "all three frames persisted");
        assert_eq!(got[0].seq, 0);
        assert_eq!(got[1].seq, 1);
        assert_eq!(got[2].seq, 2);
        assert_eq!(got[0].target.as_deref(), Some("cargo:stdout"));
        assert_eq!(got[0].message, "a");
        assert_eq!(got[1].level, LogLevel::Warn);
        assert_eq!(got[2].message, "c");
    }

    #[test]
    fn build_then_run_share_seq_and_clock() {
        // The monotonic-seq test: two sinks over ONE shared counter +
        // clock (the build phase then the run phase). Seqs must be
        // contiguous 0..M+N-1, the first M carry cargo:* targets, and
        // ts_us must be non-decreasing across the boundary.
        let (_d, db) = temp_db();
        let id = seed_job(&db);
        let seq = Arc::new(AtomicU64::new(0));
        let job_start = Instant::now();

        let mut build = FrameSink::new(id, JobLogsBroker::new(), db.clone(), seq.clone(), job_start);
        build.push(LogLevel::Info, Some("cargo:stdout".into()), "compiling".into());
        build.push(LogLevel::Info, Some("cargo:stderr".into()), "warning: x".into());
        build.finish();

        let mut run = FrameSink::new(id, JobLogsBroker::new(), db.clone(), seq.clone(), job_start);
        run.push(LogLevel::Info, None, "hello".into());
        run.push(LogLevel::Info, None, "Test OK".into());
        run.finish();

        let got = rows(&db, &id);
        let seqs: Vec<u64> = got.iter().map(|f| f.seq).collect();
        assert_eq!(seqs, vec![0, 1, 2, 3], "contiguous seqs across the boundary");
        assert!(got[0].target.as_deref().unwrap().starts_with("cargo:"));
        assert!(got[1].target.as_deref().unwrap().starts_with("cargo:"));
        assert_eq!(got[2].target, None);
        assert_eq!(got[3].target, None);
        for w in got.windows(2) {
            assert!(w[1].ts_us >= w[0].ts_us, "ts_us non-decreasing");
        }
    }

    #[test]
    fn push_publishes_to_broker() {
        let (_d, db) = temp_db();
        let id = seed_job(&db);
        let broker = JobLogsBroker::new();
        let mut rx = broker.subscribe(id);
        let mut sink =
            FrameSink::new(id, broker, db.clone(), Arc::new(AtomicU64::new(0)), Instant::now());
        sink.push(LogLevel::Info, None, "live".into());

        match rx.try_recv() {
            Ok(LiveEvent::Frame(f)) => {
                assert_eq!(f.seq, 0);
                assert_eq!(f.message, "live");
            }
            other => panic!("expected a Frame on the broker, got {other:?}"),
        }
        sink.finish();
    }

    #[test]
    fn final_flush_persists_partial_batch() {
        // Fewer than BATCH_MAX frames; only finish() forces them out.
        let (_d, db) = temp_db();
        let id = seed_job(&db);
        let mut sink = FrameSink::new(
            id,
            JobLogsBroker::new(),
            db.clone(),
            Arc::new(AtomicU64::new(0)),
            Instant::now(),
        );
        sink.push(LogLevel::Info, None, "1".into());
        sink.push(LogLevel::Info, None, "2".into());
        sink.finish();
        assert_eq!(rows(&db, &id).len(), 2);
    }
}
```

- [ ] **Step 3: Run the `FrameSink` tests**

Run: `cargo test -p paavod --lib log_sink`
Expected: 4 tests PASS (`push_assigns_monotonic_seq_and_persists`, `build_then_run_share_seq_and_clock`, `push_publishes_to_broker`, `final_flush_persists_partial_batch`).

If `JobLogsBroker::subscribe` returns a type without `try_recv`, note it is a `tokio::sync::broadcast::Receiver`, which has `try_recv()` — no async runtime needed because the frame was published synchronously before the `try_recv` call.

- [ ] **Step 4: Commit**

```bash
git add crates/paavod/src/log_sink.rs crates/paavod/src/lib.rs
git commit -m "feat(paavod): FrameSink — shared seq/ts_us/publish/batch-persist core"
```

---

## Task 3: Wire the build forwarder to `FrameSink`

Replace the build forwarder's local-seq `LogFrame` construction with a `FrameSink` fed from the shared counter + clock.

**Files:**
- Modify: `crates/paavod/src/dispatch.rs` (`build_or_cache` signature + build forwarder body + the `run_one_inner` call site)

- [ ] **Step 1: Change `build_or_cache`'s signature**

In `crates/paavod/src/dispatch.rs`, find:

```rust
fn build_or_cache(state: &AppState, job: &paavo_db::JobRow) -> Result<std::path::PathBuf, String> {
```

Replace with:

```rust
fn build_or_cache(
    state: &AppState,
    job: &paavo_db::JobRow,
    log_seq: &std::sync::Arc<std::sync::atomic::AtomicU64>,
    job_start: std::time::Instant,
) -> Result<std::path::PathBuf, String> {
```

- [ ] **Step 2: Replace the build-forwarder thread body with a `FrameSink`**

Still in `build_or_cache`, find the build-forwarder block (the `let (build_tx, build_rx) = ...` through the `.expect("spawn paavod-build-forwarder thread");`). Replace the whole block with:

```rust
    let (build_tx, build_rx) = crossbeam_channel::unbounded::<paavo_build::BuildLine>();
    let job_id_for_fwd = job.id;
    let mut sink = crate::log_sink::FrameSink::new(
        job_id_for_fwd,
        state.job_logs.clone(),
        state.db.clone(),
        log_seq.clone(),
        job_start,
    );
    let build_fwd = std::thread::Builder::new()
        .name("paavod-build-forwarder".into())
        .spawn(move || {
            // recv_timeout (not recv) so the FrameSink's 50ms flush
            // deadline fires even when cargo goes quiet mid-compile.
            loop {
                match build_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(bl) => {
                        let target = match bl.stream {
                            paavo_build::BuildStream::Stdout => "cargo:stdout",
                            paavo_build::BuildStream::Stderr => "cargo:stderr",
                        };
                        sink.push(
                            paavo_proto::LogLevel::Info,
                            Some(target.to_string()),
                            bl.text,
                        );
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => sink.tick(),
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        sink.finish();
                        break;
                    }
                }
            }
        })
        .expect("spawn paavod-build-forwarder thread");
```

(The `build_started` `Instant` local that previously existed is gone — `job_start` is the shared clock now. Remove any now-unused `build_started` binding the compiler flags.)

- [ ] **Step 3: Update the `run_one_inner` call to `build_or_cache`**

In `run_one_inner`, find:

```rust
    let elf_path = match build_or_cache(state, job) {
```

Replace with:

```rust
    let elf_path = match build_or_cache(state, job, &log_seq, job_start) {
```

- [ ] **Step 4: Build and run existing tests**

Run: `cargo test -p paavod`
Expected: PASS. The dispatch fixtures cache-bypass the build so the forwarder body is not exercised here, but compilation + the existing dispatch behavior must stay green. (Persistence behavior is covered by Task 2's `FrameSink` tests.)

- [ ] **Step 5: Commit**

```bash
git add crates/paavod/src/dispatch.rs
git commit -m "feat(paavod): build forwarder persists via FrameSink (shared seq/clock)"
```

---

## Task 4: Wire the run forwarder to `FrameSink`

`RealRunner::run` now binds `log_seq` + `job_start` from `ctx` and feeds the run forwarder's frames through a `FrameSink`.

**Files:**
- Modify: `crates/paavod/src/real_runner.rs`

- [ ] **Step 1: Bind the shared state from `ctx`**

In `crates/paavod/src/real_runner.rs`, find the destructure added in Task 1:

```rust
        let paavo_core::RunContext { job_id, board_id, .. } = ctx;
```

Replace with:

```rust
        let paavo_core::RunContext {
            job_id,
            board_id,
            log_seq,
            job_start,
        } = ctx;
```

- [ ] **Step 2: Replace the run-forwarder thread body with a `FrameSink`**

Find the run-forwarder block (around lines 195–203):

```rust
        let broker = self.job_logs.clone();
        let fwd = std::thread::Builder::new()
            .name("paavod-log-forwarder".into())
            .spawn(move || {
                while let Ok(frame) = log_rx.recv() {
                    broker.publish(job_id, LiveEvent::Frame(frame));
                }
            })
            .expect("spawn log forwarder thread");
```

Replace with:

```rust
        let mut sink = crate::log_sink::FrameSink::new(
            job_id,
            self.job_logs.clone(),
            self.db.clone(),
            log_seq,
            job_start,
        );
        let fwd = std::thread::Builder::new()
            .name("paavod-log-forwarder".into())
            .spawn(move || {
                // recv_timeout so the 50ms flush deadline fires even when
                // the firmware is quiet between defmt frames. The probe's
                // own seq/ts_us are discarded — FrameSink reassigns both
                // from the shared counter + clock.
                loop {
                    match log_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                        Ok(frame) => sink.push(frame.level, frame.target, frame.message),
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => sink.tick(),
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                            sink.finish();
                            break;
                        }
                    }
                }
            })
            .expect("spawn log forwarder thread");
```

If `LiveEvent` is now unused in `real_runner.rs`, remove it from the imports (the compiler will flag it). Confirm `crossbeam_channel` is already a dependency in scope (it is — `log_tx`/`log_rx` come from `crossbeam_channel::unbounded()`).

- [ ] **Step 3: Build and run paavod tests**

Run: `cargo test -p paavod`
Expected: PASS. (FakeRunner replaces the run path in dispatch tests, so this body is not exercised there; the change must compile and keep existing tests green.)

- [ ] **Step 4: Commit**

```bash
git add crates/paavod/src/real_runner.rs
git commit -m "feat(paavod): run forwarder persists via FrameSink; reassigns seq + ts_us"
```

---

## Task 5: Proxy `since_seq` filter (TDD)

The bandwidth trim: `ndjson_to_sse` drops `Frame` events with `seq <= since_seq`; `stream_job` reads the param.

**Files:**
- Modify: `crates/paavo-web/tests/proxy.rs` (new test)
- Modify: `crates/paavo-web/src/proxy.rs` (param + filter)

- [ ] **Step 1: Write the failing test**

In `crates/paavo-web/tests/proxy.rs`, add this test at the end of the file (it reuses the existing `spawn_fake_paavod`, `paavo_web_router`, `fetch_sse_body`, `ndjson_line` helpers):

```rust
#[tokio::test]
async fn since_seq_filters_historicals_from_sse_stream() {
    // Upstream replays frames seq 1..=10 then a terminal. A viewer that
    // already rendered through seq 5 (SSR) reconnects with ?since_seq=5;
    // the proxy must drop frames 1..=5 and emit only 6..=10 + terminal.
    use paavo_proto::{JobOutcome, LogFrame, LogLevel, WireMessage};
    let mut body = String::new();
    for seq in 1..=10u64 {
        body.push_str(&ndjson_line(&WireMessage::Frame {
            frame: LogFrame {
                seq,
                ts_us: seq * 1000,
                level: LogLevel::Info,
                target: None,
                message: format!("line {seq}"),
            },
        }));
    }
    body.push_str(&ndjson_line(&WireMessage::Terminal {
        outcome: JobOutcome::Passed,
    }));
    // spawn_fake_paavod takes &'static str; leak the body (test-only).
    let body: &'static str = Box::leak(body.into_boxed_str());

    let (addr, _g) = spawn_fake_paavod(body).await;
    let (_dir, app) = paavo_web_router(addr);
    let sse = fetch_sse_body(
        app,
        "/api/jobs/01ARZ3NDEKTSV4RRFFQ69G5FAV/stream?since_seq=5",
    )
    .await;

    for seq in 1..=5u64 {
        assert!(
            !sse.contains(&format!("line {seq}")),
            "frame {seq} should have been filtered; body:\n{sse}"
        );
    }
    for seq in 6..=10u64 {
        assert!(
            sse.contains(&format!("line {seq}")),
            "frame {seq} should have passed through; body:\n{sse}"
        );
    }
    assert!(
        sse.contains("event: terminal\n"),
        "terminal must still pass through; body:\n{sse}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p paavo-web --test proxy since_seq_filters_historicals_from_sse_stream`
Expected: FAIL — the proxy does not yet read `since_seq`, so frames 1..=5 are present.

- [ ] **Step 3: Add the `since_seq` param to `stream_job` and thread it into `ndjson_to_sse`**

In `crates/paavo-web/src/proxy.rs`, change the `stream_job` handler to read the query. Update the signature:

```rust
pub async fn stream_job(
    State(s): State<AppState>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
```

Right after the job-id validation block, parse the param. Model it as
`Option<u64>`: **absent = no filtering**, present = drop `seq <= cut`.
Do NOT default to `0` — that would drop seq 0 on an unparameterized
connect.

```rust
    // Absent param => no filtering. Present => drop frames with
    // seq <= since_seq (the SSR-covered prefix). See C2 spec §5.2.
    let since_seq: Option<u64> = q.get("since_seq").and_then(|v| v.parse().ok());
```

Find the call:

```rust
    let sse_stream = ndjson_to_sse(upstream_bytes);
```

Replace with:

```rust
    let sse_stream = ndjson_to_sse(upstream_bytes, since_seq);
```

- [ ] **Step 4: Add the filter to `ndjson_to_sse`**

Change the function signature to take the optional cutoff:

```rust
fn ndjson_to_sse<S, E>(
    s: S,
    since_seq: Option<u64>,
) -> impl futures::Stream<Item = Result<Event, Infallible>>
```

In the `Ok(WireMessage::Frame { frame })` arm, drop filtered frames
before the enrichment/yield. Non-Frame events (Phase, Lagged, Terminal,
Truncated) are never filtered.

```rust
                            Ok(WireMessage::Frame { frame }) => {
                                // Bandwidth trim: the SSR pre-populate already
                                // rendered frames through `since_seq`; don't
                                // re-ship them. Client-side lastSeq dedup is
                                // the correctness backbone; this just saves
                                // bytes on the initial connect.
                                if let Some(cut) = since_seq {
                                    if frame.seq <= cut {
                                        continue;
                                    }
                                }
                                let display_ts = crate::time::relative_us(frame.ts_us, false);
                                let payload = json!({
                                    "seq": frame.seq,
                                    "ts_us": frame.ts_us,
                                    "display_ts": display_ts,
                                    "level": frame.level,
                                    "target": frame.target,
                                    "message": frame.message,
                                    "phase": current_phase,
                                });
                                yield Ok(named_event("frame", payload));
                            }
```

`ndjson_to_sse` has exactly one caller (`stream_job`), so no other call
site needs updating. The existing proxy tests reach `ndjson_to_sse`
through the router with no `since_seq` query, so they exercise the
`None` (no-filter) path and stay green.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p paavo-web --test proxy`
Expected: PASS — the new test plus all pre-existing proxy tests (the `None` default means the existing tests, which pass no `since_seq`, are unaffected).

- [ ] **Step 6: Commit**

```bash
git add crates/paavo-web/src/proxy.rs crates/paavo-web/tests/proxy.rs
git commit -m "feat(paavo-web): proxy filters Frame events with seq <= since_seq"
```

---

## Task 6: `data-since-seq` on the log pane (TDD)

SSR computes the max persisted seq and emits it so the client knows where the pre-populated prefix ends.

**Files:**
- Modify: `crates/paavo-web/tests/smoke.rs` (seeded test)
- Modify: `crates/paavo-web/src/pages/job_detail.rs`

- [ ] **Step 1: Write the failing test**

In `crates/paavo-web/tests/smoke.rs`, add a test that seeds a job + frames and asserts the attribute. It seeds via `paavo_db` against the same sqlite path `fresh_app` uses; if `fresh_app` does not expose the path, replicate its construction inline as shown:

```rust
#[tokio::test]
async fn job_detail_emits_data_since_seq_when_frames_exist() {
    use paavo_db::{Db, LogFrameDb};
    use paavo_proto::{
        BoardSelector, JobId, JobSource, LogFrame, LogLevel, Priority,
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    let id = JobId::new();
    paavo_db::JobRow::insert(
        db.raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: Priority::Interactive,
            submitter: "x".into(),
            source: JobSource::Cli,
            board_selector: BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    let frames: Vec<LogFrame> = (0..3u64)
        .map(|seq| LogFrame {
            seq,
            ts_us: seq * 1000,
            level: LogLevel::Info,
            target: Some("cargo:stdout".into()),
            message: format!("l{seq}"),
        })
        .collect();
    LogFrame::append_batch(db.raw_conn(), &id, &frames).unwrap();
    drop(db);

    let webdb = paavo_web::db::WebDb::open(&path).unwrap();
    let paavod =
        paavo_web::proxy::PaavodClient::new("http://127.0.0.1:9").expect("valid url");
    let state = paavo_web::proxy::AppState { db: webdb, paavod };
    let app = paavo_web::app::build_router(state);

    let (status, body) = fetch(app, &format!("/jobs/{id}")).await;
    assert_eq!(status, 200);
    assert!(
        body.contains(r#"data-since-seq="2""#),
        "expected data-since-seq=\"2\" (max of seqs 0,1,2); body:\n{body}"
    );
}
```

If `smoke.rs` already imports a `fetch` helper with a different signature, match it; the existing tests in the file show the exact `fetch(app, uri)` shape used here.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p paavo-web --test smoke job_detail_emits_data_since_seq_when_frames_exist`
Expected: FAIL — `data-since-seq` is not emitted yet.

- [ ] **Step 3: Emit `data-since-seq` in `job_detail.rs`**

In `crates/paavo-web/src/pages/job_detail.rs`, find the pane-opening line:

```rust
    body.push_str(&format!(
        r#"<pre id="logpane" class="logpane" data-job-id="{id}">"#,
        id = super::html_escape(&job.id.to_string())
    ));
```

Replace with (compute the max seq from the already-loaded `logs`, emit the attribute only when frames exist):

```rust
    let since_seq_attr = logs
        .iter()
        .map(|f| f.seq)
        .max()
        .map(|m| format!(r#" data-since-seq="{m}""#))
        .unwrap_or_default();
    body.push_str(&format!(
        r#"<pre id="logpane" class="logpane" data-job-id="{id}"{since}>"#,
        id = super::html_escape(&job.id.to_string()),
        since = since_seq_attr,
    ));
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p paavo-web --test smoke`
Expected: PASS — the new test plus the existing `job_detail_page_wires_live_log_consumer` (which early-returns on the empty-DB "not found" path, unaffected).

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-web/src/pages/job_detail.rs crates/paavo-web/tests/smoke.rs
git commit -m "feat(paavo-web): emit data-since-seq on #logpane from max persisted seq"
```

---

## Task 7: `live-log.js` — dedup, phase fallback, `since_seq`

No JS test harness; verified by the manual smoke (Task 9 DoD) and the server-side tests above. Three small edits to one file.

**Files:**
- Modify: `crates/paavo-web/src/assets/live-log.js`

- [ ] **Step 1: Initialize `lastSeq` and append `since_seq` to the EventSource URL**

In `crates/paavo-web/src/assets/live-log.js`, find:

```js
  // Track terminal state so onerror's auto-reconnect doesn't keep
  // hammering the server after the stream has closed cleanly.
  let closed = false;

  const es = new EventSource('/api/jobs/' + encodeURIComponent(jobId) + '/stream');
```

Replace with:

```js
  // Track terminal state so onerror's auto-reconnect doesn't keep
  // hammering the server after the stream has closed cleanly.
  let closed = false;

  // Highest seq already rendered. Initialized from the SSR-emitted
  // data-since-seq (the last frame baked into the page). Every frame
  // with seq <= lastSeq is dropped, making the consumer idempotent
  // under historical replay, the broadcast-buffer race, and reconnects.
  let lastSeq = parseInt(pane.dataset.sinceSeq || '-1', 10);
  if (Number.isNaN(lastSeq)) lastSeq = -1;

  // Pass since_seq upstream so the proxy doesn't re-ship the SSR prefix
  // on the initial connect. Only meaningful when the page baked in
  // historical frames.
  const sinceQuery =
    pane.dataset.sinceSeq != null ? '?since_seq=' + encodeURIComponent(pane.dataset.sinceSeq) : '';
  const es = new EventSource(
    '/api/jobs/' + encodeURIComponent(jobId) + '/stream' + sinceQuery
  );
```

- [ ] **Step 2: Dedup + phase fallback in the `frame` handler**

Find the `frame` listener:

```js
  es.addEventListener('frame', function (e) {
    let f;
    try {
      f = JSON.parse(e.data);
    } catch (_err) {
      console.warn('paavo-web live-log: bad frame JSON', e.data);
      return;
    }
    // Phase tag on the line: from the proxy's enrichment if
    // present, otherwise none. The phase determines colour via the
    // matching `.phase-build` / `.phase-run` CSS class declared in
    // style.css.
    const phaseClass = f.phase ? 'phase-' + f.phase : '';
    const lvlClass = 'lvl-' + (f.level || 'info');
    const tag = f.phase ? '[' + f.phase + ']\u00a0' : '';
```

Replace down through the `const tag` line with:

```js
  es.addEventListener('frame', function (e) {
    let f;
    try {
      f = JSON.parse(e.data);
    } catch (_err) {
      console.warn('paavo-web live-log: bad frame JSON', e.data);
      return;
    }
    // Idempotency: drop any frame we've already rendered. Closes the
    // historical-replay, broadcast-buffer-race, and reconnect dup
    // sources with one mechanism (see the C2 spec §5).
    if (typeof f.seq === 'number') {
      if (f.seq <= lastSeq) return;
      lastSeq = f.seq;
    }
    // Phase tag: prefer the proxy's enrichment; fall back to an EXACT
    // inference from target (cargo:* => build, else => run) so
    // stream-replayed historical frames — which carry no Phase events —
    // tag identically to SSR-rendered ones.
    const phase =
      f.phase || (f.target && f.target.indexOf('cargo:') === 0 ? 'build' : 'run');
    const phaseClass = 'phase-' + phase;
    const lvlClass = 'lvl-' + (f.level || 'info');
    const tag = '[' + phase + ']\u00a0';
```

(The rest of the handler — `ts`, `lvl`, `html`, `appendLine` — is unchanged and continues to use `phaseClass`, `lvlClass`, and `tag`.)

- [ ] **Step 3: Build paavo-web to confirm the asset still serves**

Run: `cargo build -p paavo-web`
Expected: PASS. (`live-log.js` is a static asset; this confirms nothing references a renamed symbol. JS behavior is verified in the Task 9 manual smoke.)

- [ ] **Step 4: Commit**

```bash
git add crates/paavo-web/src/assets/live-log.js
git commit -m "feat(paavo-web): live-log.js dedups by lastSeq + infers phase from target"
```

---

## Task 8: Retention note in the deployment doc

**Files:**
- Modify: `docs/deployment.md`

- [ ] **Step 1: Add the retention paragraph**

Open `docs/deployment.md`. Find the section that discusses retention / `passed_full_log_days` (search for `passed_full_log_days`). Immediately after the existing description of that setting, add:

```markdown
> **Build output is Info-level.** Both build-phase log lines
> (`target = cargo:stdout` / `cargo:stderr`) and most run-phase frames
> are persisted at `info` level, so they are eligible for the
> `passed_full_log_days` sweep — a passed job's full log is deleted that
> many days after it finishes, while `warn`/`error` frames are kept
> indefinitely. Operators who want to retain complete build logs for
> passed jobs permanently can set `passed_full_log_days = -1`, which
> disables truncation entirely.
```

If `docs/deployment.md` has no `passed_full_log_days` section yet, add a short `## Log retention` section at the end containing the paragraph above (without the leading `>` blockquote markers).

- [ ] **Step 2: Commit**

```bash
git add docs/deployment.md
git commit -m "docs(deployment): note build lines are info-level + retention behavior"
```

---

## Task 9: Final verification + cleanup

**Files:**
- Delete: `docs/superpowers/plans/2026-06-15-c2-persist-log-frames.md` (superseded)

- [ ] **Step 1: Remove the superseded informal plan**

```bash
git rm docs/superpowers/plans/2026-06-15-c2-persist-log-frames.md
```

- [ ] **Step 2: Full workspace gate**

Run: `cargo fmt --all`
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean (no warnings).

Run: `cargo test --workspace`
Expected: PASS — baseline counts plus the new `FrameSink` tests, the proxy `since_seq` test, and the smoke `data-since-seq` test.

- [ ] **Step 3: Commit any formatting + the plan removal**

```bash
git add -A
git commit -m "chore(c2): remove superseded plan; fmt"
```

- [ ] **Step 4: Manual smoke (definition of done — requires a real EVK)**

1. Rebuild `paavod` + `paavo-web`; start both.
2. Submit a job; let it run to terminal.
3. Open `/jobs/<id>` in a **fresh** browser tab AFTER the terminal.
4. Verify: the log pane is populated with build lines (`[build]` tag) and run lines (`[run]` tag); timestamps are monotonic across the build→run boundary; and opening the page does NOT double-render the historical chunk (the `EventSource` connecting adds no duplicates).
5. Reload the page mid-stream of a fresh job and confirm no duplication after reconnect.

---

## Notes for the executor

- **Task order matters.** Task 1 (trait change) must land first and keep the build green by updating all five implementors at once. Task 2 (`FrameSink`) must precede Tasks 3–4 (which consume it).
- **`since_seq` semantics:** absent param = no filtering (model as `Option<u64>`); present = drop `seq <= since_seq`. Do not default to `0`, which would drop seq 0.
- **`JobId` is `Copy`** — pass it by value freely (the forwarders already do).
- **`JobLogsBroker::subscribe`** returns a `tokio::sync::broadcast::Receiver`; use `try_recv()` in sync unit tests.
- **DB lock:** `FrameSink::flush` holds the global `state.db` mutex for one `append_batch` transaction. This is the accepted contention point (spec §7); keep the lock scope to the transaction only.
