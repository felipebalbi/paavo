# Design: startup reconciliation of orphaned jobs

**Status**: design approved 2026-06-16. Small, self-contained feature.
Adds one `AbortReason` wire variant; no schema migration.

---

## 1. Problem

If paavod dies while a job is mid-flight — crash, `kill`, or being
stopped during a build/run — the job's row is left in `building` or
`running` state. A freshly-started paavod has no worker thread for that
row, so it sits there forever:

- `paavo-cli admin purge` refuses (409) whenever any job is `building`
  or `running`, so a single orphaned row **permanently deadlocks the
  purge** (no `--force` flag exists).
- The web UI shows a job stuck "running" that will never finish.

This was hit in practice: paavod was killed mid-build, and the orphaned
`building` row had to be cleared with raw `sqlite3` surgery before
`admin purge` would run again.

## 2. Solution

On startup, before the dispatch loop starts, paavod scans for jobs in
`building`/`running` and terminalizes each as
`Aborted { by: Interrupted }`, appending a forensic log line. This makes
restart **self-healing**: orphaned jobs become terminal, `admin purge`
works again, and the UI shows an honest "interrupted" outcome.

Because there is exactly one paavod process (single-writer assumption,
spec §7), any `building`/`running` row present at startup is provably
orphaned — no live worker can own it — so terminalizing all of them is
safe.

## 3. Decisions

### 3.1 Outcome family: `Aborted`, not `Failed(InfraErr)`

An orphaned job is terminalized as `JobOutcome::Aborted`, never
`Failed(TerminalOutcome::InfraErr)`.

*Why:* `JobOutcome::counts_toward_infra_failure()` returns `true` only
for `InfraErr`. `apply_outcome_to_board` bumps the board's consecutive-
infra-failure counter (and auto-quarantines at the threshold) solely on
that. A daemon crash is **not** a board failure — using `InfraErr` would
wrongly quarantine working hardware every time paavod restarts mid-job.
`Aborted` does not count, so boards are untouched. (Verified in
`paavo-core/src/quarantine.rs` + `paavo-proto/src/job.rs`.)

### 3.2 New `AbortReason::Interrupted`

`AbortReason` today has `User` (cli cancel) and `DaemonShutdown`
(SIGTERM drain ran out of grace). Add a third, `Interrupted`, serialized
as `"interrupted"`, for the startup-reconciliation case.

*Why a new variant, not reuse:* `DaemonShutdown` means "a clean drain
timed out." A crash/kill-and-recover is a different cause; operators
must be able to tell them apart. The whole point of the feature is
operator-visible self-healing, so the reason is worth a small wire
addition.

*Blast radius (verified):* every existing `AbortReason` reference in the
workspace is a constructor (`by: AbortReason::User` /
`DaemonShutdown`); nothing matches `AbortReason` exhaustively. The web
UI and paavo-cli render the `by` field via serde, not via match arms.
So the variant is purely additive — no consumer code needs a new arm.

### 3.3 Forensic log frame

On reconcile, append one `Warn`-level `log_frame` to each orphaned job
so the web log pane ends with an explicit explanation rather than
stopping mid-output. Message:

> `job interrupted: paavod restarted while this job was in-flight; any output above is partial`

## 4. Behavior & placement

`main.rs`, immediately after `Db::open(...)` and before
`dispatch::spawn(...)` (so the dispatch loop never sees stale in-flight
rows, and the DB is consistent before serving):

```rust
let n = paavo_db::JobRow::abort_interrupted_jobs(
    db.raw_conn(),
    Utc::now().timestamp_millis(),
)?;
if n > 0 {
    tracing::warn!(
        reconciled = n,
        "startup: aborted orphaned in-flight jobs (interrupted)"
    );
}
```

- `submitted` jobs are **left untouched** — they are queued, not
  orphaned; the scheduler re-dispatches them normally.
- Already-terminal jobs are untouched.
- Startup-time `warn!` gives operational visibility (`reconciled=N`).

## 5. The DB method

New method in `paavo-db` (`crates/paavo-db/src/job.rs`, next to
`finalize`):

```rust
/// Terminalize every job still in `building`/`running` as
/// `Aborted { by: Interrupted }`, appending a forensic Warn frame to
/// each. Returns the count reconciled. Idempotent: a second call
/// finds nothing and returns 0. Intended to run once at daemon
/// startup, before the dispatch loop — any in-flight row at that
/// point is provably orphaned.
pub fn abort_interrupted_jobs(conn: &Connection, now_ms: i64) -> Result<u64>
```

Implementation — one transaction over the whole sweep so a crash
mid-sweep commits nothing and a re-run is clean:

1. `SELECT id, started_at FROM job WHERE state IN ('building','running')`.
2. For each orphaned row:
   a. `seq = SELECT COALESCE(MAX(seq), -1) + 1 FROM log_frame WHERE job_id = ?`
      — lands after whatever was captured before the crash.
   b. `ts_us = max(0, now_ms - started_at) * 1000` (saturating into
      `u64`; `started_at` is non-NULL for building/running rows in
      practice, but fall back to `0` if NULL). Continues the job's
      relative timeline.
   c. `INSERT INTO log_frame (job_id, seq, ts_us, level, target, message)`
      with `level='warn'`, `target=NULL`, the message from §3.3.
   d. `UPDATE job SET state='aborted', outcome_detail=?, finished_at=?`
      where `outcome_detail` is
      `serde_json::to_string(&JobOutcome::Aborted { by: AbortReason::Interrupted })`
      and `finished_at = now_ms`.
3. Commit. Return the count.

This is a dedicated method rather than a loop over the existing
`finalize` + `append_batch` helpers because those each open their own
transaction (sqlite has no nested transactions); doing the raw
INSERT/UPDATE inside one sweep transaction keeps the whole operation
atomic and idempotent.

## 6. Wire change

`crates/paavo-proto/src/job.rs`: add to `AbortReason`:

```rust
/// paavod restarted while this job was still building/running; the
/// startup reconciliation pass terminalized the orphaned row.
Interrupted,
```

It inherits the enum's existing `#[serde(rename_all = "snake_case")]`,
serializing to `"interrupted"`; the full outcome wire shape is
`{"aborted":{"by":"interrupted"}}`.

## 7. Testing

1. **`paavo-db`** (`tests` or inline): seed a DB with one `building`,
   one `running`, one `submitted`, and one terminal (`passed`) job; give
   the `running` job two pre-existing `log_frame` rows (seq 0,1). Run
   `abort_interrupted_jobs(now)`. Assert:
   - returns `2`;
   - both in-flight jobs are now `aborted` with
     `outcome_detail` = `{"aborted":{"by":"interrupted"}}`;
   - each got exactly one new `Warn` forensic frame, and the `running`
     job's forensic frame is at `seq = 2`;
   - the `submitted` and `passed` jobs are unchanged (state + frames);
   - a second call returns `0` and changes nothing (idempotent).
2. **`paavo-proto`** (`serde_roundtrip` + `wire_compat`):
   `AbortReason::Interrupted` round-trips and the
   `Aborted { by: Interrupted }` outcome serializes to
   `{"aborted":{"by":"interrupted"}}`.

The `main.rs` call site is a three-line wiring of the tested method +
a log line; it is not separately unit-tested (consistent with how the
rest of `main.rs` startup wiring is treated).

## 8. Scope — out

- **No config toggle.** Always-on self-healing; there is no scenario
  where leaving orphaned rows stuck is desirable.
- **No change to the SIGTERM-drain path.** `DaemonShutdown` still covers
  the clean drain-timeout case; `Interrupted` is specifically the
  crash/kill-and-recover case discovered at startup.
- **No reconciliation of `submitted`.** Those are not orphaned.
- **No `admin purge --force` flag.** Reconciliation removes the need:
  after restart there are no stuck in-flight rows, so the existing
  guard is no longer a deadlock.

## 9. Cosmetic note

The forensic frame has `target = NULL`, so the web UI's phase inference
(`cargo:*` → build, else → run) tags it `[run]`. This is harmless — the
`Warn` level color makes the line stand out regardless — and not worth
a special-case in the phase logic. Recorded as a known, accepted
detail.

## 10. Files touched

| File | Change |
| --- | --- |
| `crates/paavo-proto/src/job.rs` | Add `AbortReason::Interrupted` |
| `crates/paavo-db/src/job.rs` | New `JobRow::abort_interrupted_jobs` |
| `crates/paavod/src/main.rs` | Call it after `Db::open`, before `dispatch::spawn`; warn-log the count |
| `crates/paavo-db/tests/*` | Reconciliation test |
| `crates/paavo-proto/tests/serde_roundtrip.rs` + `wire_compat.rs` | `Interrupted` wire assertions |

**Migration:** none. **Estimated size:** ~120–160 LOC including tests.
