# Design: persist build + run log frames to `log_frame`

**Status**: design approved 2026-06-16. Backend write-path + a focused
frontend dedup/phase change. No schema migration; no wire (`WireMessage`
/ `LogFrame`) shape change.

**Supersedes**: the informal plan at
`docs/superpowers/plans/2026-06-15-c2-persist-log-frames.md`. That plan
identified the gap correctly but misdiagnosed two things — the shared-state
seam (it named `paavo_runner::JobInputs`; the real seam is paavo-core's
`Runner` trait) and the de-duplication strategy (server-side `since_seq`
alone leaves two dup sources open). This spec is the corrected, approved
design; the implementation plan derived from it replaces the old plan
file.

---

## 1. Goal

Persist both build-phase and run-phase log frames to the `log_frame`
table so that **historical replay works**: a viewer who opens
`/jobs/:id` after a job has finished — or whose `EventSource`
reconnects mid-flight — sees the complete log, not an empty pane.

Live streaming (paavod broker → SSE proxy → browser `EventSource`)
already works end-to-end. This design adds the **write path** and
closes the de-duplication and phase-tagging gaps that persistence
exposes.

## 2. Background: the precise gap

As of the paavo-web overhaul (commits `5cc0ab7..9c9ed71`):

| Producer | Publishes to live broker | Persists to `log_frame` |
| --- | --- | --- |
| Build forwarder (`dispatch.rs::build_or_cache`) | yes | **no** |
| Run forwarder (`real_runner.rs::RealRunner::run`) | yes | **no** |

Consequences:

- **Live tail works.** A viewer connected before/during the job sees
  every frame via the broadcast channel.
- **After-the-fact view is empty.** A viewer opening a completed job's
  page sees an empty `<pre id="logpane">` — the daemon emitted hundreds
  of frames but none were written.
- **Build-failure forensics are partial.** The failing rustc diagnostic
  is in `outcome_detail` (last 8 KiB of stderr via `BuildErr`), but
  every preceding line is gone.

The **read path is already complete**: `LogFrame::list` pages rows,
`stream_job::historical_lines` emits them as wire frames before
live-tailing, and `job_detail.rs` SSR-renders them into the initial
HTML. Only the **write path** is missing — plus the dedup/phase
consequences of turning it on.

## 3. Decisions

Four decisions were made during design review, each with the rejected
alternatives recorded so the rationale survives.

### 3.1 De-duplication: client seq-dedup is the backbone; `since_seq` is a bandwidth trim

Once both forwarders persist, a frame can reach the browser by up to
three paths (see §5). Server-side `since_seq` filtering alone (the old
plan) closes only one of them. The decision: make **`live-log.js`
idempotent** by tracking `lastSeq` and dropping any frame with
`seq <= lastSeq`. This closes all three dup sources with one mechanism.
`since_seq` is kept as a server-side optimization that avoids
re-shipping the SSR-covered prefix on the initial connect.

*Rejected:* "server `since_seq` only" (leaves the stream-internal
buffer-race dup and the full-log reconnect re-render open — a
known-latent double-render bug that activates the moment persistence
lands). *Rejected:* "client dedup only, drop `since_seq`" (correct, but
wastes bandwidth re-shipping the SSR prefix on every fresh page load of
a large terminal job).

### 3.2 Phase tags on replayed frames: infer from `target`

Per-line `[build]`/`[run]` tagging infers from `target`:
`cargo:*` → build, everything else → run. This is an **exact** mapping,
not a heuristic — build frames always carry `cargo:stdout`/`cargo:stderr`;
run frames carry defmt module paths or `None`, never `cargo:`. The
inference goes in `live-log.js` as a fallback when the proxy-supplied
`f.phase` is absent, matching what SSR already does at
`job_detail.rs:119`. `Phase` SSE events keep driving the top
phase-banner; only per-frame tagging changes.

*Why needed:* `historical_lines` replays frames only — no `Phase`
events — so the proxy's `current_phase` cursor stays `None` during
replay and stream-replayed frames would render with `phase: null` and
no tag. SSR frames are fine (they infer from `target`); the JS just
didn't.

*Rejected:* "infer in the proxy, drop the cursor" (cleaner single
source of truth but a larger change to working code; deferred as
unnecessary). *Rejected:* "persist/replay `Phase` events" (most code,
plus a semantics question about reconstructing phase boundaries from
persisted rows).

### 3.3 Timestamp origin: one shared monotonic clock, restamp both phases

Both forwarders stamp `ts_us` from a single `Instant` captured at the
top of `run_one_inner` (job-execution start, just before the build).
Build frames climb `0..Tbuild`; run frames continue from `Tbuild`
onward. One monotonic timeline across the whole job.

*Why:* the old plan's prose claimed monotonic time but its code kept
each phase on its own clock — build relative to `build_started`, run
relative to the probe session start (which begins well after the build).
That makes the rendered `mm:ss.fff` column **reset** at the build→run
boundary. The probe's `ts_us` is host decode-time relative to session
start, not the firmware clock, so it carries no precision worth
preserving over a shared host timeline.

*Rejected:* "offset probe `ts_us` by build duration" (monotonic and
preserves intra-run spacing, but more plumbing for precision that isn't
the firmware clock). *Rejected:* "keep per-phase clocks" (timestamps
visibly reset; reads as a bug).

### 3.4 Shared-state seam: `RunContext` through the `Runner` trait

The shared seq counter and job-start `Instant` are created in
`dispatch::run_one_inner` — the common parent of `build_or_cache` (build
forwarder) and `runner.run` (run forwarder). They reach the build
forwarder as plain params and the run forwarder bundled into a
`RunContext` passed through paavo-core's `Runner::run`.

*Why this seam:* the old plan named `paavo_runner::JobInputs`, but
`JobInputs` feeds the *worker* (`paavo_runner::run_job`); the run
forwarder that reassigns seq lives paavod-side in `RealRunner::run`. The
forwarder does not need anything on `JobInputs`. The real abstraction
boundary between dispatch and the runner is paavo-core's `Runner` trait
(`fn run(&self, job_id, board_id) -> RunOutcome`). Changing it to
`fn run(&self, ctx: RunContext<'_>) -> RunOutcome` is a
paavod-internal-ish change — `RealRunner` (the one production impl that
uses the new fields) plus four implementors that read `ctx.job_id` /
`ctx.board_id` and ignore the rest: a dev `FakeRunner` in
`crates/paavod/src/main.rs` and three test doubles (`FakeRunner`,
`PanickyRunner`, `CountingRunner`) in
`crates/paavod/tests/dispatch_loop.rs`. Five implementors total, all in
paavod; paavo-core defines the trait + the new `RunContext` struct.

*Rejected:* "DB-derived seq base + wall-clock origin, no trait change."
The run forwarder would read `count_for_job` for its base seq (exact,
since phases are sequential) and stamp `ts_us` against a wall-clock
origin. But every clean wall-clock origin is awkward: `started_at` is
set *post-build* (build frames get negative `ts_us`), `submitted_at`
folds in queue-wait time, and a purpose-built origin needs a new column
— the schema change the work was trying to avoid. The trait change is
the smaller, cleaner cost.

## 4. Architecture

### 4.1 Shared state

`dispatch::run_one_inner`, at the top, before the build:

```
let job_start = std::time::Instant::now();
let log_seq   = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
```

- `build_or_cache(state, job, &log_seq, job_start)` — new params for the
  build forwarder.
- `RunContext { job_id, board_id, log_seq, job_start }` — handed to
  `Runner::run(ctx)` for the run forwarder.

`RunContext` is defined in `paavo-core` next to `RunOutcome`:

```
pub struct RunContext<'a> {
    pub job_id: paavo_proto::JobId,
    pub board_id: &'a str,
    pub log_seq: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub job_start: std::time::Instant,
}
```

### 4.2 Sequential-phase invariant (correctness foundation)

`build_or_cache` joins its forwarder thread (`build_fwd.join()`) before
returning. `run_one_inner` then transitions Building→Running and only
afterward calls `runner.run`, which spawns the run forwarder. The two
forwarders therefore never run concurrently — there is a thread-join
happens-before edge between them.

Consequences:
- The `Arc<AtomicU64>` is never accessed concurrently. `fetch_add(1,
  Relaxed)` is correct.
- Build emits seqs `0..B-1`; run continues `B..B+R-1`. The
  `(job_id, seq)` primary key can never collide.

### 4.3 Per-frame persistence contract

Each forwarder thread, per frame:

1. `seq = log_seq.fetch_add(1, Relaxed)` — authoritative, overrides any
   probe-side seq.
2. `ts_us = u64::try_from(job_start.elapsed().as_micros()).unwrap_or(u64::MAX)`
   — shared monotonic timeline.
3. `broker.publish(job_id, LiveEvent::Frame(frame.clone()))` — live path,
   unchanged.
4. Push into a batch; flush via `LogFrame::append_batch` every **64
   frames or 50 ms**, whichever comes first, plus a final flush when the
   source channel closes (`RecvTimeoutError::Disconnected`).

The build forwarder switches from `crossbeam_channel::recv()` to
`recv_timeout(50ms)` so the 50 ms flush deadline fires even when cargo
goes quiet mid-compile. The run forwarder makes the same change.

`flush_batch` helper:

```
fn flush_batch(db: &Arc<Mutex<Db>>, job_id: &JobId, batch: &[LogFrame]) {
    let conn = db.lock();
    if let Err(e) = LogFrame::append_batch(conn.raw_conn(), job_id, batch) {
        tracing::error!(error = %e, %job_id,
            "log forwarder: append_batch failed; frames lost from history");
    }
}
```

### 4.4 What is and isn't persisted

- **Persisted:** build lines (`target = cargo:stdout|cargo:stderr`,
  level Info) and run frames (defmt; `target` = module path or `None`;
  real levels).
- **Not persisted:** `Phase` events (encoded by `job.state` +
  `started_at`/`finished_at`; a stream-only signal for live viewers) and
  the `Terminal` event (lives in `outcome_detail`). Unchanged.

## 5. De-duplication

Three dup sources, and what closes each:

| # | Source | Closed by |
| --- | --- | --- |
| 1 | SSR pre-populates frames `0..N` into the HTML `<pre>`; the stream's `historical_lines` replays them | `since_seq=N` server trim **and** client `seq <= lastSeq` drop |
| 2 | `stream_job` subscribes to the broker before `historical_lines` reads the DB; a frame both persisted and broadcast-buffered in that window is emitted twice in one stream | client `seq <= lastSeq` drop |
| 3 | `EventSource` auto-reconnects → `historical_lines` replays the whole log; `since_seq` is frozen at the page-load value | client `seq <= lastSeq` drop |

### 5.1 Client dedup (`live-log.js`)

```
let lastSeq = parseInt(pane.dataset.sinceSeq || '-1', 10);
// in the `frame` handler, before rendering:
if (f.seq <= lastSeq) return;
lastSeq = f.seq;
```

Makes the consumer idempotent under any replay — the property the
current stateless consumer lacks.

### 5.2 `since_seq` (bandwidth trim)

- `job_detail.rs` SSR computes `max_seq = logs.iter().map(|f| f.seq).max()`
  and emits `data-since-seq="{max_seq}"` on `#logpane`. Omitted when no
  historical rows exist.
- `live-log.js` reads `dataset.sinceSeq` and appends it:
  `/api/jobs/{id}/stream?since_seq={n}`.
- `proxy.rs::stream_job` accepts `Query<HashMap<String,String>>`, reads
  `since_seq`, and forwards it onto the upstream paavod URL.
- paavod `stream_job` accepts `since_seq`; `historical_lines` skips rows
  with `seq <= since_seq`.

`since_seq` trims only the initial connect. Reconnect replays from the
frozen value; client dedup absorbs it. Chasing `lastSeq` on reconnect
(recreating the `EventSource` with an updated `since_seq`) is a possible
future bandwidth optimization — **out of scope**; client dedup makes it
unnecessary for correctness.

### 5.3 SSR cap interaction (verified clean)

SSR uses `LogFrame::list(offset=0, limit=2000)` — the *first* 2000
frames (head, ascending by seq). For jobs ≤2000 frames, SSR shows
everything and `since_seq` = max; the stream adds only newer live
frames. For jobs >2000, SSR shows `0..1999`, `data-since-seq=1999`, and
the stream delivers `2000..end` via `historical_lines`. Head SSR +
tail-of-historical stream composes to the full log with no gap and no
dup.

## 6. Phase tags

Per-line tagging infers from `target` (exact mapping; see §3.2):

```
const phase = f.phase || (f.target && f.target.startsWith('cargo:') ? 'build' : 'run');
```

Then `phase` drives both the `[build]`/`[run]` tag text and the
`phase-build`/`phase-run` CSS class. SSR-rendered and stream-replayed
frames tag identically. The proxy's `current_phase` cursor and the
`Phase`→banner path are untouched.

## 7. Failure modes & retention

- **DB-write failure (`flush_batch`):** log at `error` and continue. A
  failed flush leaves a gap in the historical view but must never abort
  the build or run; the live broker already delivered every frame.
- **DB-lock contention:** `flush_batch` takes the global `state.db`
  mutex per flush — the same lock dispatch/HTTP/scheduler share. The
  64-frame/50 ms batching bounds this to ~20 short transactions/sec
  during a noisy build; SQLite WAL keeps each commit fast. Accepted for
  v1; no connection-pool work.
- **Retention:** `truncate_old_passed` already deletes
  `level IN (trace,debug,info)` for passed jobs older than
  `passed_full_log_days`; build lines are Info, so they are swept, while
  warn/error frames are kept indefinitely. `docs/deployment.md` gains a
  paragraph noting build output is Info-level (hence sweep-eligible) and
  that `passed_full_log_days = -1` disables truncation for operators who
  want permanent build logs.

## 8. Testing strategy

1. `paavod/tests/dispatch_loop.rs` —
   `build_and_run_lines_share_monotonic_seq`: a `FakeRunner` emits N run
   frames after a build that emits M build lines; assert `log_frame`
   rows have contiguous seqs `0..M+N-1` with no duplicates, the first M
   carry `target = cargo:*`, and `ts_us` is non-decreasing across the
   boundary.
2. `paavod/tests/api_jobs.rs` —
   `historical_replay_after_terminal_returns_persisted_frames`: run a
   fake job to terminal, then `GET /jobs/:id/stream` and assert the
   persisted historical frames come back (read path over real rows).
3. `paavo-web/tests/proxy.rs` —
   `since_seq_filters_historicals_from_sse_stream`: seed upstream NDJSON
   frames seq `1..10`, request `?since_seq=5`, assert only `6..10` are
   emitted as SSE `frame` events.
4. `paavo-web/tests/smoke.rs` — extend the job-detail smoke to seed a
   few frames and assert `data-since-seq` appears on `#logpane`.
5. The `live-log.js` `lastSeq` dedup and `target`-phase fallback are
   covered by the proxy/SSR tests plus the §9 manual smoke. We do not
   stand up a JS test harness for this change.

## 9. Definition of done

- `cargo test -p paavo-core -p paavod -p paavo-web` green; full
  workspace `fmt` + `clippy` clean.
- Manual smoke against a real EVK: submit a job, let it complete, then
  open `/jobs/<id>` in a fresh tab **after** the terminal. The log pane
  is populated with build lines (`[build]` tag) and run lines (`[run]`
  tag); timestamps are monotonic across the boundary; and the
  `EventSource` opening does not double-render the historical chunk.
- `docs/deployment.md` notes the build-line retention behavior.

## 10. Scope — explicitly out

- `TerminalOutcome::BuildErr.stderr` → `summary` rename + cap shrink. A
  wire change touching `wire_compat.rs`; the 8 KiB tail still fits
  `outcome_detail`. Separate PR if the outcome-card UI ever wants the
  shorter form.
- Reconnect-time `since_seq` chasing (recreate `EventSource` with an
  updated `lastSeq`). Pure bandwidth; client dedup covers correctness.
- Persisting `Phase` / `Terminal` rows. Encoded elsewhere; unchanged.

## 11. Files touched

| File | Change |
| --- | --- |
| `crates/paavo-core/src/runner.rs` | Define `RunContext<'a>`; change `Runner::run` to take it |
| `crates/paavod/src/dispatch.rs` | Create `job_start` + `log_seq`; pass to `build_or_cache`; build `RunContext`; build forwarder persists + batches + restamps seq/ts_us |
| `crates/paavod/src/real_runner.rs` | `RealRunner::run(ctx)`; run forwarder reassigns seq, restamps ts_us, persists + batches (via `self.db`) |
| `crates/paavod/src/main.rs` | Update the dev `FakeRunner` impl to `run(ctx)` |
| `crates/paavod/src/routes/jobs.rs` | `stream_job` reads `since_seq`; `historical_lines` skips `seq <= since_seq` |
| `crates/paavod/tests/dispatch_loop.rs` | Update three test doubles to `run(ctx)`; new monotonic-seq test |
| `crates/paavod/tests/api_jobs.rs` | Historical-replay-after-terminal test |
| `crates/paavo-web/src/proxy.rs` | `since_seq` query param → upstream URL |
| `crates/paavo-web/src/pages/job_detail.rs` | Compute max seq → `data-since-seq` |
| `crates/paavo-web/src/assets/live-log.js` | `lastSeq` dedup; `target`-inferred phase fallback; `since_seq` on URL |
| `crates/paavo-web/tests/proxy.rs` | `since_seq` filter test |
| `crates/paavo-web/tests/smoke.rs` | Seed frames; assert `data-since-seq` |
| `docs/deployment.md` | Retention note on build lines |

**Migration:** none. **Proto change:** none — `RunContext` is
paavo-core-internal; `WireMessage` and `LogFrame` wire shapes are
unchanged. **Estimated size:** ~300–400 LOC including tests.
