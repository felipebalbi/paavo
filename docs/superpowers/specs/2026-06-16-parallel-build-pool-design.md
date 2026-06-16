# Design: parallel build pool + board-decoupled run stage

**Status**: design approved 2026-06-16. Medium feature; one new job
state, one DB migration, a two-stage dispatcher.

---

## 1. Problem

On a small board fleet, jobs serialize far more than the hardware
requires. The dispatcher claims a board *before* building and holds it
for the **entire build+run**:

1. `pick_next` (`paavo-core/src/scheduler.rs`) only returns a job when a
   matching healthy board is free.
2. `transition_to_building` claims that board (`board_id` set on the
   row).
3. The blocking worker runs `cargo build` (minutes) **and** the probe
   run, both while holding the board.
4. `find_healthy_for_selector` (`paavo-db/src/board.rs:92`) excludes any
   board with a `building`/`running` job, so no second job can start.

The build phase needs **no hardware** — only the run phase attaches a
probe. Yet with one board, a second submission sits in `Submitted` until
the first job's *build and run* both finish. From an operator's seat
"paavod is busy after the first submission."

What is **not** broken today (verified, and must stay that way):

- **Acceptance is already concurrent.** `POST /jobs`
  (`paavod/src/routes/jobs.rs:54`) streams the tar, inserts a row,
  returns `202`; it never waits on build/run, and the shared
  `db.lock()` is held sub-millisecond per insert.
- **`paavo-cli run` is fire-and-forget by default**
  (`cmd_run.rs:52`) — it returns once the upload is accepted; only
  `--follow` streams to terminal.

So `for i in $(seq 1 10); do paavo-cli run …; done` already produces 10
accepted rows. The gap is purely **execution concurrency**.

## 2. Goal

Let many jobs make progress at once on a small fleet by **decoupling the
`cargo build` phase from board occupancy**, bounded by a global build
cap. Concretely:

- Up to `max_concurrent_builds` (default **5**) builds run in parallel,
  each in its own `CARGO_TARGET_DIR`.
- A board is claimed **only** for the run phase.
- Submissions beyond the cap are accepted and wait; never rejected.

### 2.1 Invariants

- **INV-1 (acceptance ≠ execution):** every `POST /jobs` is accepted
  (`202`) and persisted regardless of in-flight load. The cap gates
  execution only. 10 concurrent submits → 10 rows; up to 5 build, the
  rest wait in `Submitted`.
- **INV-2 (build parallelism):** up to `max_concurrent_builds`
  `cargo build` processes run truly in parallel — each with a distinct
  `CARGO_TARGET_DIR` so cargo's per-target-dir file lock cannot
  serialize them.
- **INV-3 (board exclusivity):** at most one job in `Running` per
  board (unchanged from today).
- **INV-4 (board-free build):** a board is claimed at `→ Running`, never
  during `Building`/`AwaitingBoard`.

## 3. Resource model & two-stage scheduler

Two scarce resources, scheduled independently each dispatch tick:

| Stage | Gated by | Claim (DB transition) |
| --- | --- | --- |
| **Build** | a free build slot (in-memory pool of `max_concurrent_builds`, §3.1) | `Submitted → Building` |
| **Run** | a free healthy matching board (existing LRU pick) | `AwaitingBoard → Running` |

Each tick runs the **run stage first** (keep the scarcer resource —
boards — busy), then fills build slots:

```
loop {
    if draining { drain-exit when no build/run in flight }

    // Run stage: drain AwaitingBoard onto free boards.
    while let Some((job, board)) = pick_runnable(...) {
        claim: AwaitingBoard -> Running (sets board_id)
        spawn_blocking run_one_run(job, board)
    }

    // Build stage: fill free build slots.
    while let Some(slot) = slots.try_acquire() {
        let Some(job) = pick_buildable(...) else { slots.release(slot); break };
        claim: Submitted -> Building (sets started_at)
        spawn_blocking run_one_build(job, slot)   // releases slot when done
    }

    sleep(tick)
}
```

The pick + claim happen under the existing `db.lock()`, so "next
buildable job" + "take it" is atomic against other transitions. Build
concurrency is bounded by an **in-memory slot pool** of
`max_concurrent_builds` entries owned by the (single) dispatch task;
each slot maps 1:1 to a target dir `build-slots/<i>` (§5.1). A build
occupies its slot from claim until its task finishes, then releases it.

### 3.1 Why an in-memory slot pool

The dispatcher is the sole party that starts builds (single-writer,
spec §7), so an in-memory pool is the simplest correct bound — and it
assigns the target dir in the same step. It is crash-safe **because
startup reconciliation (§9.3) runs first**: any orphaned `building` row
is terminalized before the loop starts, so the pool begins fully free
and always matches reality (`occupied slots == COUNT(state='building')`).
A `tokio::sync::Semaphore` or a live DB `COUNT` would add shared state
without buying anything under the single-dispatcher assumption.

## 4. State machine

Add exactly **one** state, `AwaitingBoard`. `Submitted` stays the
capacity queue (it already behaves as one). The wire/DB value is
`awaiting_board`; the UI keeps showing `Submitted` (no display rename).

```
                    build slot free
        Submitted ───────────────────▶ Building
          │                               │
          │ cancel (inline)               ├── build error ──▶ Failed(BuildErr)
          ▼                               │
       Aborted(User)                      │ build OK
                                          ▼
                                    AwaitingBoard ───────────────▶ Running ──▶ terminal
                                          │        board free        │
                                          │ cancel (inline)          │ (probe run, watchdog,
                                          ▼                          │  cancel, timeouts — unchanged)
                                    Aborted(User)                    ▼
                                                              Passed / Failed /
                                                              TimedOut / Aborted
```

### 4.1 `paavo-proto` (`src/job.rs`)

Add the variant, mirroring the existing `#[serde(rename = …)]` table:

```rust
/// Built; ELF ready; waiting for a free matching board. The build
/// slot has been released — this job no longer counts toward
/// `max_concurrent_builds`.
#[serde(rename = "awaiting_board")]
AwaitingBoard,
```

`JobState::is_terminal()` is unchanged (`AwaitingBoard` is non-terminal).
Wire-compat + serde round-trip tests get an `awaiting_board` case.

### 4.2 `paavo-db` transitions (`src/job.rs`)

The board moves from the build claim to the run claim:

- **`transition_to_building`** — drop the `board_id` argument. Becomes
  `Submitted → Building`, setting `started_at` only. No `board_id` (the
  build holds no board); the slot index is tracked in-memory by the
  dispatcher (§3.1), not persisted.

  ```sql
  UPDATE job SET state='building', started_at=?
  WHERE id=? AND state='submitted'
  ```

- **`transition_to_awaiting_board`** — **new**. `Building →
  AwaitingBoard`, records the stable ELF path (§5.2).

  ```sql
  UPDATE job SET state='awaiting_board', elf_path=?
  WHERE id=? AND state='building'
  ```

- **`transition_to_running`** — now `AwaitingBoard → Running`, setting
  `board_id` (the run claim). `elf_path` was already set at
  `awaiting_board`, so it no longer takes an elf argument.

  ```sql
  UPDATE job SET state='running', board_id=?
  WHERE id=? AND state='awaiting_board'
  ```

- **`finalize`** — extend the guard from
  `state IN ('submitted','building','running')` to also include
  `awaiting_board`, so a cancel/abort of an `AwaitingBoard` job is a
  valid terminalization.

### 4.3 Board-exclusion query (`paavo-db/src/board.rs:92`)

Today `find_healthy_for_selector` excludes boards whose job is
`building` **or** `running`. Since `building` jobs no longer hold a
board (`board_id` is NULL until `→ Running`), narrow the exclusion to
the state that actually owns a board:

```sql
AND NOT EXISTS (
    SELECT 1 FROM job WHERE job.board_id = board.id
      AND job.state = 'running'
)
```

This is both correct and clearer. (Leaving `IN ('building','running')`
would also work — a `building` row has `board_id IS NULL` and matches no
`board.id` — but the narrowed form states the intent.)

## 5. Build slots & artifact stability

### 5.1 Per-slot target dirs

`StateDir` (`paavod/src/state_dir.rs`) replaces the single shared
`cargo_target_dir` with a **pool of N slot dirs**:

```
<state_dir>/build-slots/0/   ← CARGO_TARGET_DIR for slot 0
<state_dir>/build-slots/1/
…
<state_dir>/build-slots/<max_concurrent_builds - 1>/
```

- Slot count == `max_concurrent_builds`. The dispatcher assigns a build
  the lowest free slot index from its in-memory pool (§3.1), so a
  claimed build always has a dedicated target dir.
- Slots are **reused** across builds, preserving incremental compilation
  *within a slot* (shared deps like `embassy-mcxa` rebuild only on a
  slot's first cold use). This is the trade we accepted for true
  parallelism: deps may compile up to N times across the pool instead of
  once, in exchange for builds that never block on cargo's target-dir
  lock.
- `ensure_dirs(n)` creates the slot dirs; `n` comes from config.

`BuildPlan.target_dir` (`paavo-build/src/build.rs`) is set to the
claimed slot's dir per build; `paavo-build` itself is unchanged. Stable
ELF artifacts live in the existing `cache_elfs_dir` (§5.2), never in a
slot dir.

### 5.2 Stable ELF artifact

`paavo-build` discovers the ELF inside the slot's target dir. Because
slots are reused, that path is volatile — worse, two different jobs
scaffolded from the template often share a crate (binary) name (e.g.
`hello-mcxa266`), so the next build in the same slot can **overwrite the
same path** with different firmware. `cache_store`
(`paavo-core/src/build_cache.rs`) only records the path *string*, so a
cached `elf_path` pointing into a reused slot could later resolve to the
**wrong ELF**. That is a correctness hazard, not just untidiness.

Fix: after a successful build, copy the discovered ELF to a stable,
content-addressed path in the **existing** cache dir and record *that*:

```
<state_dir>/cache/elf/<tar_blake3>.elf     (StateDir.cache_elfs_dir)
```

- `elf_path` (job row) and `cache_store` both receive this path. Content
  addressing by `tar_blake3` means identical content → identical path
  (safe dedupe), different content → different path (no collision).
- It is a **copy**, not a move, so the slot keeps its incremental state
  for the next build.
- This integrates with the existing build-cache LRU for free:
  `evict_lru` already `remove_file`s `entry.elf_path`, and `cache_lookup`
  already self-heals a missing file to `Miss`. (Today `cache_store` is
  handed the shared-target-dir path; this change routes it through the
  cache dir it was always meant to own.)
- Because the artifact persists independently of the slot, an
  `AwaitingBoard` job **survives a daemon restart** and still runs
  (§9.3). If the artifact is missing at run-claim time (evicted/crash),
  the run stage treats it as a cache miss and re-queues the job to
  `Submitted` for a rebuild.

### 5.3 Single-flight by `tar_blake3`

To stop N identical concurrent submits from building the same tar N
times, the **build picker skips** a `Submitted` job whose `tar_blake3`
is already in `Building`:

```sql
-- pick_buildable: next Submitted job whose tar_blake3 is NOT
-- currently building (priority, then oldest first).
SELECT * FROM job j WHERE j.state='submitted'
  AND NOT EXISTS (
    SELECT 1 FROM job b
    WHERE b.state='building' AND b.tar_blake3 = j.tar_blake3)
ORDER BY j.priority ASC, j.submitted_at ASC
LIMIT 1
```

Result for `for i in $(seq 1 10); do paavo-cli run <same args>; done`:
job 1 builds; jobs 2–10 stay `Submitted` until job 1 stores the cache,
then each cache-hits straight to `AwaitingBoard` and serializes on the
board. Distinct jobs are unaffected (different blake3 → buildable in
parallel). Skip-cache jobs (`skip_cache=true`) still respect
single-flight on the *build* (so two identical `--skip-cache` submits
don't double-compile concurrently) but bypass the cache lookup as today.

## 6. Build picking, run picking, starvation

`paavo-core/src/scheduler.rs` splits `pick_next` into two pure reads:

- **`pick_buildable(conn, cfg, now) -> Option<JobRow>`** — the
  single-flight query in §5.3, with the existing **starvation
  promotion** applied first (a `Scheduled` job older than
  `starvation_threshold_ms` is treated as `Interactive` for ordering).
  No board involved.
- **`pick_runnable(conn, cfg, now) -> Option<ScheduledJob>`** — among
  `AwaitingBoard` jobs in priority/age order, return the first that has
  a free healthy matching board (existing LRU `lru_pick`). Starvation
  promotion applies here too so a long-waiting scheduled job isn't
  perpetually out-ranked for a board.

`MAX_SUBMITTED_SCAN` bounding is preserved for both.

## 7. Concurrency cap & config

`[scheduler]` (`paavod/src/config.rs`) gains:

```rust
/// Max concurrent `cargo build` processes (each gets its own
/// CARGO_TARGET_DIR). Jobs beyond this wait in `Submitted`.
#[serde(default = "default_max_concurrent_builds")]
pub max_concurrent_builds: usize,
```

```rust
fn default_max_concurrent_builds() -> usize { 5 }
```

- `paavo-core::SchedulerConfig` gains `max_concurrent_builds: usize`
  (threaded from `paavod` config, like `starvation_threshold_ms`).
- `sample-paavo.toml` documents the knob.
- Build-ahead is otherwise **unbounded** in v1: the build stage keeps
  slots full whenever `Submitted` work exists, even if boards are
  scarce. A `max_built_ahead` guard (cap on `AwaitingBoard` depth) is
  noted as a future knob, not built now (§12).

## 8. Acceptance guarantee (INV-1)

No change to the HTTP path is required — it is already non-blocking and
unbounded (modulo `max_upload_bytes` and the drain 503). The work here
is to **lock it in with a test**: fire 10 concurrent `POST /jobs` and
assert 10 distinct accepted rows, independent of how many can build. The
cap lives entirely in the dispatcher; the ingress never consults it.

## 9. Lifecycle edges

### 9.1 Cancellation

- **`Submitted`** — inline `Aborted(User)` (today's
  `cancel_if_submitted`), unchanged.
- **`AwaitingBoard`** — inline `Aborted(User)`. No worker, no board; same
  shape as the `Submitted` cancel. `cancel_if_submitted` generalizes to
  `cancel_if_pending` covering both pre-run states (the
  `finalize` guard already permits `awaiting_board` per §4.2).
- **`Building`** — **kill the cargo child.** The build worker registers
  its `std::process::Child` (or a kill handle) in a per-job registry
  keyed by `JobId`; `POST /jobs/:id/cancel` for a `Building` job signals
  it, the worker kills the child, and the build path returns
  `Aborted(User)` instead of advancing to `AwaitingBoard`. This mirrors
  the existing `CancellationRegistry` pattern used for the run watchdog,
  but carries a process-kill capability rather than a `RunCommand`
  sender. If no live build handle exists (already finished, race), the
  handler falls through to the existing 409/registry logic.
- **`Running`** — unchanged (watchdog `RunCommand::Cancel`).

### 9.2 `admin purge` guard

`paavod/src/routes/admin.rs` refuses purge while jobs are in-flight.
Extend the guard from `state IN ('building','running')` to include
`awaiting_board` — those jobs own a build artifact and are mid-flight.
The error string and `paavo-cli`/test expectations update accordingly
("building, awaiting board, or running").

### 9.3 Startup reconciliation

`abort_interrupted_jobs` (per the 2026-06-16 reconciliation design)
terminalizes orphaned `building`/`running` rows as
`Aborted { by: Interrupted }`. Under the new model:

- **`building`** orphan → still `Interrupted` (its cargo child died with
  the daemon; the partial slot target dir is untrusted).
- **`running`** orphan → still `Interrupted` (unchanged).
- **`awaiting_board`** orphan → **left intact**. No worker was lost; the
  job was merely waiting, and its ELF artifact (§5.2) persists. The
  dispatch loop re-enters it into the run stage on startup. (If the
  artifact is gone, the run stage re-queues it to `Submitted`.)

So `abort_interrupted_jobs` keeps scanning only `('building','running')`
— no change to that query — and we add a test asserting an
`awaiting_board` row is untouched across the sweep.

### 9.4 SIGTERM drain

`dispatch` stops picking **new** builds and runs the moment
`drain.is_draining()` is set, and exits once nothing is in flight.
"In flight" now spans both stages. Drain-exit waits until
`COUNT(state IN ('building','running')) == 0` (builds and runs both
quiesce); the build-cancel handles registered in §9.1 plus the run
`CancellationRegistry` are signalled by `drain_with_grace`, so a grace
timeout turns lingering work into `Aborted(DaemonShutdown)`. The
existing `cancellation.active()`-based wait is extended to also account
for in-flight builds, tracked via the build-cancel handle registry
(which empties as builds finish).

## 10. Files touched

| File | Change |
| --- | --- |
| `crates/paavo-proto/src/job.rs` | Add `JobState::AwaitingBoard` (`"awaiting_board"`) |
| `crates/paavo-proto/tests/{serde_roundtrip,wire_compat}.rs` | `awaiting_board` assertions |
| `crates/paavo-db/migrations/V2__awaiting_board.sql` | Rebuild `job` table with the extended `state` CHECK (adds `awaiting_board`) |
| `crates/paavo-db/src/job.rs` | `state_to_str`/`state_from_str` arm; split transitions (§4.2); `finalize` guard; pick-support queries |
| `crates/paavo-db/src/board.rs` | Narrow exclusion to `state='running'` (§4.3) |
| `crates/paavo-core/src/scheduler.rs` | `pick_buildable` + `pick_runnable` (replace `pick_next`); single-flight |
| `crates/paavo-core/src/cancel.rs` | `cancel_if_pending` (Submitted + AwaitingBoard) |
| `crates/paavod/src/state_dir.rs` | `build_slots_dir` pool (drops single `cargo_target_dir`); `ensure_dirs(n)` |
| `crates/paavod/src/config.rs` | `scheduler.max_concurrent_builds` (default 5) |
| `crates/paavod/src/dispatch.rs` | Two-stage loop; per-slot builds; artifact copy; build-cancel registry |
| `crates/paavod/src/real_runner.rs` | Read `board_id` set at `→Running`; no behavior change to the run itself |
| `crates/paavod/src/routes/jobs.rs` | `parse_state` arm `awaiting_board`; cancel routes to `cancel_if_pending` / build-kill |
| `crates/paavod/src/routes/admin.rs` | Purge guard includes `awaiting_board` |
| `crates/paavod/src/cancellation.rs` | Build-kill handle registry (alongside run cancel) |
| `crates/paavo-cli/src/*`, `crates/paavo-web/src/*` | Render `awaiting_board` (display label unchanged: "Submitted" stays "Submitted") |
| `sample-paavo.toml` | Document `max_concurrent_builds` |

## 11. Testing

`paavo-core` / `paavo-db` (deterministic, no hardware):

- **Cap** — with `max_concurrent_builds=N` and a fake builder that
  blocks on a barrier, assert at most N rows are `building` at once.
- **Build-while-board-busy** — a job builds while another job occupies
  the only board (`Running`); proves INV-4.
- **Run still serialized** — never two `Running` on one board (INV-3),
  re-using the existing board-exclusion tests.
- **Single-flight** — two identical `tar_blake3` submits → exactly one
  build; the second reaches `AwaitingBoard` via cache hit.
- **Pick order** — `pick_buildable`/`pick_runnable` honor priority +
  starvation promotion.
- **`cancel_if_pending`** — cancelling `AwaitingBoard` → `Aborted(User)`.
- **Reconciliation** — `abort_interrupted_jobs` leaves an
  `awaiting_board` row intact; still aborts `building`/`running`.

`paavod` (integration, `PAAVO_FAKE_RUNNER` + a fake/fast builder):

- **10-submits-all-accepted** — 10 concurrent `POST /jobs` → 10 rows
  (INV-1).
- **End-to-end** — submit > N distinct jobs against 1 board; assert all
  reach a terminal state, builds overlapped (peak `building` > 1), runs
  serialized.
- **Building-cancel** — cancel a job mid-build kills the child and
  terminalizes `Aborted(User)`.
- **Drain** — SIGTERM during overlapping build+run quiesces both within
  grace.

## 12. Scope — out (v1)

- **No `max_built_ahead` knob.** Build-ahead is unbounded; revisit if a
  large fleet imbalance wastes CPU/disk building far ahead of board
  capacity.
- **No per-user fairness beyond existing priority + starvation.** "Multi
  user" is served by concurrent acceptance + the build pool; round-robin
  per submitter is a later concern.
- **No display rename.** `Submitted` keeps its label in UI/CLI;
  `awaiting_board` renders as itself.
- **No change to the run phase** (probe attach, watchdog, timeouts,
  quarantine) beyond where the board is claimed.

## 13. Migration

`V2__awaiting_board.sql`. SQLite cannot `ALTER` a `CHECK` constraint, so
the `job` table is rebuilt following the standard table-redefinition
procedure: `PRAGMA foreign_keys=OFF`, then inside a transaction create
`job_new` with the extended `state` CHECK (adding `awaiting_board`),
`INSERT … SELECT *` all rows, `DROP TABLE job`, `ALTER TABLE job_new
RENAME TO job`, recreate any indexes; commit; `PRAGMA foreign_keys=ON`.
No data transformation — existing rows already carry valid states. The
`log_frame.job_id` foreign key (`ON DELETE CASCADE`) references `job` by
name and stays satisfied after the rename. The plan verifies how
`Db::open` runs migrations and places the `PRAGMA` toggles accordingly.

**Estimated size:** ~500–700 LOC including tests across 6 crates.
