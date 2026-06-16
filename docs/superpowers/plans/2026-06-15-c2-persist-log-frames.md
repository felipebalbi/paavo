# C2 follow-up: persist build + run frames to `log_frame`

**Status**: deferred from the paavo-web overhaul shipped 2026-06-15 (commits
`5cc0ab7` through `9c9ed71`). Backend-only; no UI changes; no schema
changes.

**Why deferred**: keeping this out of the live-streaming work let the UI
ship first. The streaming pipeline (paavod broker → SSE proxy → browser
EventSource) works end-to-end today; what's missing is **historical
replay**. A subscriber that joins `/jobs/:id/stream` AFTER the job
finished sees nothing, because `log_frame` is empty in production.

This document is the design + checklist to close that gap. Read it on
another machine, port over the work, ship.

---

## 1. The gap, precisely

Today, in the paavod codebase as of `9c9ed71`:

| Producer | Path | Persists to `log_frame`? |
| --- | --- | --- |
| Build forwarder (`dispatch.rs::build_or_cache`) | broker `LiveEvent::Frame` | **No** |
| Run forwarder (`real_runner.rs`, the thread spawned around `log_rx`) | broker `LiveEvent::Frame` | **No** |
| Anywhere else | — | No |

So:

- **Live tail works**: a viewer with EventSource open from before the job
  starts sees every frame.
- **After-the-fact view is empty**: a viewer who clicks `/jobs/<some-id>`
  for a job that completed an hour ago sees an empty `<pre id="logpane">`,
  even though the daemon emitted hundreds of frames during the run.
- **Build failure forensics are partial**: the failing rustc diagnostic
  is in `outcome_detail` JSON (last 8 KiB of stderr), but every line that
  came before is gone. Operators chasing a flaky-build issue can't grep
  for warnings that preceded the error.

The schema is ready (`migrations/V1__initial.sql` defines `log_frame`
with `target TEXT` already nullable; the `target` column is exactly what
distinguishes `cargo:stdout` / `cargo:stderr` / defmt module paths /
None). `paavo_proto::LogFrame::list` already pages historical frames.
`paavod::routes::jobs::stream_job::historical_lines` already calls
`LogFrame::list` and emits the rows as wire frames before live-tailing.
Every read-path piece is in place; only the write path is missing.

## 2. The blocker that pushed this out of scope

The two forwarders are sequential phases of the same job. Both want to
write to `log_frame`. The primary key is `(job_id, seq)`. That means
the seq numbering has to span both phases — if both forwarders start at
0, the run forwarder's first insert violates the PK. So we need a
**single seq counter per job**, shared between both forwarders.

Today:

- The build forwarder is local to `dispatch.rs::build_or_cache`. It
  owns its seq counter (a plain `u64` mutated in-place inside the
  thread closure).
- The run forwarder is inside `paavod::real_runner::Runner::run`. The
  frames it forwards come pre-numbered from `paavo_probe::session::
  drain_one_frame` (see `crates/paavo-probe/src/session.rs`, where
  `RealSession.seq: u64` is bumped per emitted frame, starting at 0).
  The forwarder doesn't touch `frame.seq`.

To unify: dispatch creates an `Arc<AtomicU64>` at the top of
`run_one_inner`; passes one clone to the build forwarder, another clone
to the runner via a new field on `paavo_runner::JobInputs`. Both
forwarders `fetch_add(1, Relaxed)` per persist. The run forwarder
**reassigns** `frame.seq` to the value from the shared counter,
overriding whatever the probe emitted. (The probe's seq stays useful for
its own internal `tracing::debug!` log correlation, but the
authoritative wire-and-DB seq is now dispatch-side.)

The `paavo_runner::JobInputs` API change is the reason this needed its
own commit instead of being folded into commit C1.

## 3. Plan

### 3.1 Step 1 — paavo-runner `JobInputs` field

`crates/paavo-runner/src/lib.rs` (or wherever `JobInputs` is declared,
last seen in `paavo_runner` re-exports):

```rust
pub struct JobInputs {
    // … existing fields …
    pub job_id: paavo_proto::JobId,
    pub inactivity_timeout_ms: u64,
    pub hard_max_ms: u64,
    pub probe_release_grace_ms: u64,
    pub cancel_rx: crossbeam_channel::Receiver<()>,
    /// Shared per-job seq counter. paavod creates one
    /// `Arc<AtomicU64>` per dispatched job and hands clones to
    /// every forwarder that persists a `LogFrame` row, so build
    /// frames (paavod-side) and run frames (paavo-runner-side)
    /// land on monotonically-increasing seqs and never collide on
    /// the `(job_id, seq)` primary key. The counter starts at 0
    /// in dispatch and is incremented per persist via
    /// `fetch_add(1, Ordering::Relaxed)` — relaxed is fine because
    /// the only consumer of the value is the SQLite insert, and
    /// SQLite's own per-statement locking provides the happens-
    /// before for the rows.
    pub log_seq: std::sync::Arc<std::sync::atomic::AtomicU64>,
}
```

**Wire** (callers): `crates/paavod/src/real_runner.rs::Runner::run`
constructs `JobInputs` (~line 183 in the existing tree). Add the field
there. Tests that construct `JobInputs` for `paavo-runner`'s own unit
tests need updating — pass a fresh `Arc::new(AtomicU64::new(0))`.

`paavo-runner` doesn't change its public surface beyond adding the
field; the worker code keeps not touching `frame.seq` because the
forwarder owns that translation.

### 3.2 Step 2 — both forwarders persist + reassign seq

#### Build forwarder (paavod-side)

In `dispatch.rs::build_or_cache`, the forwarder closure:

```rust
let job_id_for_fwd = job.id;
let broker_for_fwd = state.job_logs.clone();
let db_for_fwd = state.db.clone();          // NEW
let seq_for_fwd = log_seq.clone();          // NEW — Arc<AtomicU64> from caller
let build_started = std::time::Instant::now();
let build_fwd = std::thread::Builder::new()
    .name("paavod-build-forwarder".into())
    .spawn(move || {
        // Batch frames to amortise the SQLite per-INSERT cost; flush
        // every 64 lines or 50 ms (matches the run forwarder
        // batching policy this commit also adds).
        let mut batch: Vec<LogFrame> = Vec::with_capacity(64);
        let mut last_flush = Instant::now();

        loop {
            // crossbeam_channel::recv_timeout returns Err(RecvTimeoutError::Timeout)
            // on timeout, Err(Disconnected) when build_release_streaming drops the
            // sender. The Disconnected case ends the loop after one final flush.
            match build_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(bl) => {
                    let target = match bl.stream {
                        BuildStream::Stdout => "cargo:stdout",
                        BuildStream::Stderr => "cargo:stderr",
                    };
                    let frame = LogFrame {
                        seq: seq_for_fwd.fetch_add(1, Ordering::Relaxed),
                        ts_us: u64::try_from(build_started.elapsed().as_micros())
                            .unwrap_or(u64::MAX),
                        level: LogLevel::Info,
                        target: Some(target.into()),
                        message: bl.text,
                    };
                    broker_for_fwd.publish(job_id_for_fwd, LiveEvent::Frame(frame.clone()));
                    batch.push(frame);
                    if batch.len() >= 64 || last_flush.elapsed() >= Duration::from_millis(50) {
                        flush_batch(&db_for_fwd, &job_id_for_fwd, &batch);
                        batch.clear();
                        last_flush = Instant::now();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if !batch.is_empty() {
                        flush_batch(&db_for_fwd, &job_id_for_fwd, &batch);
                        batch.clear();
                        last_flush = Instant::now();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    if !batch.is_empty() {
                        flush_batch(&db_for_fwd, &job_id_for_fwd, &batch);
                    }
                    break;
                }
            }
        }
    })
    .expect("spawn paavod-build-forwarder thread");
```

`flush_batch` is a small helper:

```rust
fn flush_batch(db: &Arc<Mutex<Db>>, job_id: &JobId, batch: &[LogFrame]) {
    let conn = db.lock();
    if let Err(e) = paavo_proto::LogFrame::append_batch(conn.raw_conn(), job_id, batch) {
        // DB write failures are operator-visible (a gap appears in
        // the historical view), but they MUST NOT abort the build.
        // Log and continue; the live broker still saw every frame.
        tracing::error!(error = %e, %job_id, "build forwarder: append_batch failed; frames lost");
    }
}
```

(`LogFrame::append_batch` is the trait method on `paavo_proto::LogFrame`
that already exists in `paavo-db/src/log.rs:42-65` — it just has no
production callers today.)

#### Run forwarder (paavod-side)

`real_runner.rs` already has the forwarder. Refactor it the same way:

```rust
let broker = self.job_logs.clone();
let db = self.db.clone();
let seq = job_inputs.log_seq.clone();
let started_at = Instant::now();
let fwd = std::thread::Builder::new()
    .name("paavod-log-forwarder".into())
    .spawn(move || {
        let mut batch: Vec<LogFrame> = Vec::with_capacity(64);
        let mut last_flush = Instant::now();
        loop {
            match log_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(mut frame) => {
                    // **Reassign** seq from the shared counter. The
                    // probe-side seq is internal to paavo-probe and
                    // not the wire-and-DB authoritative one.
                    frame.seq = seq.fetch_add(1, Ordering::Relaxed);
                    broker.publish(job_id, LiveEvent::Frame(frame.clone()));
                    batch.push(frame);
                    if batch.len() >= 64 || last_flush.elapsed() >= Duration::from_millis(50) {
                        flush_batch(&db, &job_id, &batch);
                        batch.clear();
                        last_flush = Instant::now();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if !batch.is_empty() {
                        flush_batch(&db, &job_id, &batch);
                        batch.clear();
                        last_flush = Instant::now();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    if !batch.is_empty() {
                        flush_batch(&db, &job_id, &batch);
                    }
                    break;
                }
            }
        }
    })
    .expect("spawn paavod-log-forwarder thread");
```

#### dispatch.rs creates the counter

At the top of `run_one_inner`:

```rust
let log_seq = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
```

Pass `log_seq.clone()` into the build forwarder (new local clone in
`build_or_cache`'s argument list) and into `JobInputs` for the runner.

### 3.3 Step 3 — `historical_lines` already works

No change needed in `routes/jobs.rs::historical_lines` — it pages
`LogFrame::list` from sqlite. Once the forwarders write rows, the
historical replay flows automatically.

The frontend job-detail page (`pages/job_detail.rs`) already infers the
phase tag from `target` (`cargo:*` → `[build]`, otherwise `[run]`), so
historical frames render with phase tags out of the box.

### 3.4 Step 4 — historical-vs-live deduplication

Now that BOTH the SSR-side `db.job_logs(...)` pre-populate AND the SSE
proxy's upstream stream emit historical frames, the client double-renders
the historical chunk. Two options the architect listed:

- **(a)** SSE proxy strips historicals: paavo-web's `/api/jobs/:id/stream`
  takes a `?since_seq=N` query param, paavo-web's job-detail SSR
  computes the max seq on the historical pre-populate and passes it via
  the EventSource URL. The proxy filters: only forward `Frame` events
  with `frame.seq > since_seq`. **Recommended.** Keeps the
  "page-rendered with logs even with JS off" property for terminal jobs.
- **(b)** Drop SSR pre-populate: empty pane on first paint, JS fills it.
  Simpler but loses the no-JS-readable property.

(a) is the right move. The implementation:

- `paavo-web/src/proxy.rs::stream_job` accepts `Query<HashMap<String, String>>`,
  reads `since_seq`, defaults to `0`. Inside `ndjson_to_sse`'s match arm
  for `WireMessage::Frame`, skip the yield if `frame.seq <= since_seq`.
- `paavo-web/src/pages/job_detail.rs` computes `max_seq = logs.iter().map(|f| f.seq).max().unwrap_or(0)`
  and renders the data attribute on the pane:
  `data-job-id="..." data-since-seq="42"`.
- `crates/paavo-web/src/assets/live-log.js` reads `dataset.sinceSeq` and
  appends it as a query param: `new EventSource('/api/jobs/' + jobId +
  '/stream?since_seq=' + sinceSeq)`.

Pin with a new `tests/proxy.rs` test that seeds an upstream NDJSON body
with frames at seqs 1..10, requests with `?since_seq=5`, and asserts
only seqs 6..10 are emitted as SSE `frame` events.

### 3.5 Step 5 — retention sweep

`paavo_db::LogFrameDb::truncate_old_passed` already exists and runs over
`level IN (trace, debug, info)`. Build lines at level Info are eligible,
which matches the desired policy: a passed build's output is not
interesting after 30 days. Note this in the deployment guide
(`docs/deployment.md`); operators wanting to keep the full build log
can bump `passed_full_log_days` or set it to `-1`.

### 3.6 Step 6 — tests

Net-new tests:

1. **`paavod/tests/api_jobs.rs`** — extend the historical-stream tests
   to seed rows via the new forwarder path (or, simpler, keep the
   existing fixture-seeding-by-`append_batch` tests; the production
   write path doesn't need its own test — the integration test that
   asserts rows appear after a fake-runner job runs is the meaningful
   one). Add: `dispatch_loop_persists_runtime_frames_through_forwarder`
   — fixture runs a `FakeRunner` that emits 3 frames; assert
   `log_frame` rows appear with seqs 0, 1, 2.

2. **`paavod/tests/dispatch_loop.rs`** — new test
   `build_lines_and_run_lines_share_monotonic_seq`. Fixture: a
   `FakeRunner` that emits 5 frames after the build phase. Inject 3
   build lines via a fake `paavo_build` (or use the streaming surface
   directly with a known input). Assert the resulting `log_frame`
   rows have seqs `0, 1, 2, 3, 4, 5, 6, 7` with no duplicates and the
   first three carry `target = Some("cargo:stdout"|"cargo:stderr")`
   while the last five have whatever the runner produced.

3. **`paavo-web/tests/proxy.rs`** — new test
   `since_seq_filters_historicals_from_sse_stream`. Same fixture
   shape as the existing tests; pass `?since_seq=N` and assert the
   SSE body skips lower-seq frames.

4. **`paavo-web/tests/smoke.rs`** — extend
   `job_detail_page_wires_live_log_consumer` (already exists) with an
   assertion that `data-since-seq` appears on `#logpane` when
   historical rows are present. Requires seeding the smoke fixture's
   DB with a job + a few log frames; modest fixture growth.

### 3.7 Step 7 — `BuildErr` field rename (optional)

The architect's design recommended renaming `TerminalOutcome::BuildErr.stderr`
→ `summary` and shrinking the cap from 8 KiB to ~1 KiB now that the full
build log lives in `log_frame`. **Leave this out**; the 8 KiB tail still
fits in `outcome_detail`, no operator scripts pin the field name, and a
rename is a wire-shape change that touches `wire_compat.rs`. Land it as
a separate small PR if and when paavo-web's outcome card UI wants the
shorter form.

## 4. Files touched

| File | Why |
| --- | --- |
| `crates/paavo-runner/src/lib.rs` (or wherever `JobInputs` lives) | Add `log_seq: Arc<AtomicU64>` field |
| `crates/paavod/src/dispatch.rs` | Create the counter; pass to build forwarder; pass into `JobInputs` for the runner |
| `crates/paavod/src/real_runner.rs` | Receive the counter; reassign `frame.seq`; persist via `append_batch` |
| `crates/paavo-web/src/proxy.rs` | `?since_seq=N` query param; skip lower-seq frames |
| `crates/paavo-web/src/pages/job_detail.rs` | Compute max seq from historical; emit `data-since-seq` |
| `crates/paavo-web/src/assets/live-log.js` | Read `dataset.sinceSeq`; append to SSE URL |
| `crates/paavod/tests/dispatch_loop.rs` | New monotonic-seq test |
| `crates/paavo-web/tests/proxy.rs` | New `since_seq` filter test |
| `crates/paavo-web/tests/smoke.rs` | Seed a job + frames; assert `data-since-seq` |
| `docs/deployment.md` | Note the retention policy on build lines |

No schema migration. No public proto change. Single commit, focused.

## 5. Why split this off in the first place

Three reasons, in priority order:

1. **Scope**: the live-streaming work is a UX win operators feel
   immediately. Persistence is a back-office feature that matters for
   investigations days later. Shipping the UX work first means the
   feedback loop on the new pipe is faster.
2. **API blast radius**: persistence requires a `JobInputs` field. That
   ripples into every caller of `paavo-runner` (today there's only one,
   `real_runner.rs`, but the API change is on a public seam). Bundling
   it with the streaming work would have made the latter's commit
   harder to review.
3. **Dedup**: as soon as both write paths exist, the SSR-pre-populate +
   live-tail double-render becomes visible. Better to land the dedup
   query param in the same commit that introduces the persistence,
   rather than as a follow-up to the follow-up.

## 6. Estimated size

~250-350 lines of code added across ~6 files plus tests. No new deps.
~1-2 hours of focused work plus test runs. The architect E spec under
"§4.1 Q3 — back-compat with `BuildErr.stderr`" and "§4.4 Q4 — historical-
vs-live de-duplication" cover the same ground in more detail; reread
those sections if anything here is ambiguous.

## 7. Definition of done

- `cargo test -p paavod -p paavo-runner -p paavo-web` green.
- A manual smoke against a real EVK: submit a job, let it complete, then
  load `/jobs/<that-id>` in a fresh browser tab AFTER the terminal —
  the log pane is populated with build lines (magenta `[build]` tag) and
  run lines (cyan `[run]` tag), and no double-rendering when the SSE
  connection opens because the `since_seq` filter caught the boundary.
- The retention guide in `docs/deployment.md` mentions that build lines
  are info-level and therefore eligible for the `passed_full_log_days`
  sweep.
