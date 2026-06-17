# Design: dashboard tallies from SQL aggregates (snappy at any scale)

**Status**: design approved 2026-06-17. paavo-proto + paavo-db +
paavo-web + paavo-web-ui change. **No schema migration** (the required
indexes already exist), no `WireMessage` / `LogFrame` shape change, no
paavod change, no change to the `/api/events` SSE contract.

---

## 1. Goal

The dashboard (`/`) currently derives every stat card by fetching a wide
window of rows and counting them **client-side in WASM**: a 200-row jobs
page and a 100-row boards page, then `.iter().filter().count()` over each
to produce "Running", "Queue", "Boards healthy/total", and "Pass rate".
This is O(n) per render and — worse — **silently wrong past the fetch
ceilings**: a fleet of >100 boards caps `fleet_total` at 100, and the
per-page clamp bounds every other tally to its window.

This design moves the counting to the database. The dashboard fetches a
single small, **bounded** payload — exact aggregate counts plus two short
display lists — and the browser only renders. The numbers stay correct
and the page stays snappy whether the lab has 3 boards or 30 000 jobs.

## 2. Background: where the counts come from today

`crates/paavo-web-ui/src/components/dashboard.rs::render` takes a
`Page<JobListItem>` (fetched at `per_page=200`) and a `Page<BoardView>`
(fetched at `per_page=100`) and computes, all in the browser:

- `running` = jobs in `Running`.
- `queue` = jobs in `Submitted | Building | AwaitingBoard`.
- `terminal` / `passed` → `pass_rate` = `passed / terminal`, deliberately
  scoped to the recent 200-row window ("Pass rate (recent)").
- `fleet_total` = `boards.items.len()`, `healthy` = boards in `Healthy`,
  `quarantined` = `fleet_total - healthy`.

It also renders two **lists**: the 8 newest jobs ("Recent activity") and
**one row per board** in the fetched window ("Board fleet").

Two distinct concerns are tangled here:

1. **Tallies** — must be exact over the *whole* table, not a page.
2. **Lists** — "Recent activity" is naturally bounded (8 rows); "Board
   fleet" today grows with the fleet (up to the 100 cap) and would render
   thousands of `<tr>`s as the lab scales.

The server already has the right primitives nearby: list endpoints
expose exact totals via `Page.total` (a SQL `COUNT(*)`), the `job` table
has `idx_job_state`, and the `board` table is small and id-stable. Only
the dashboard reinvents counting in the client.

## 3. Decisions

### 3.1 Aggregate in SQL, served by a dedicated endpoint (chosen: "Approach A")

Counting moves into typed paavo-db queries (`GROUP BY state`, board
health counts), exposed through one new read endpoint the dashboard
calls. This is the literal "offload counting to the database" the feature
asks for: exact at any scale, additive, migration-free, and idiomatic
(handlers already call typed paavo-db queries; `Page.total` is already a
SQL count).

*Rejected — poller-computed aggregates in `LiveState`:* the background
poller already full-scans jobs (the in-memory index) and boards each
tick, so it *could* compute tallies inline for zero extra queries. But it
moves counting **off** the DB (jobs tallies would come from the in-memory
index, not SQL — the opposite of the stated direction), couples the
numbers to the poll interval, and adds derived state to `LiveState`. More
moving parts for a win that does not matter: the SQL reads are
sub-millisecond and fire only on revision bumps.

*Rejected — push stats inside `/api/events`:* folding aggregates into the
SSE would remove the stats fetch entirely, but it pollutes the clean
"events carry only revision numbers → client refetches" contract with
payload data and complicates snapshot/reconnect. Over-engineered for one
view.

### 3.2 Pass rate becomes all-time over retained jobs

Today's "Pass rate (recent)" is `passed / terminal` over the 200-row
window — a side effect of the fetch cap, not a deliberate window. With
SQL aggregation the natural and chosen meaning is **all-time over every
retained job** (`passed / (passed + failed + timed_out + aborted)`),
bounded only by the retention window. The card label drops "(recent)".

*Rejected — a real time window (e.g. last 24 h) or "last N terminal":*
both preserve a "recent" flavour but need an extra parameter and a
windowed query for a number nobody asked to scope. All-time is the
simplest correct definition and matches the other now-exact tallies.

### 3.3 One consolidated `GET /api/dashboard`, not three calls

The endpoint returns everything the page needs in **one bounded
payload**: the aggregate counts plus the two short display lists (8
recent jobs, the relevant fleet slice). The dashboard becomes a single
`LocalResource` keyed on the jobs **and** boards revisions.

*Why:* one round-trip, one resource in the UI (today it juggles two), and
a payload whose size is fixed (~16 rows + a handful of integers)
regardless of fleet or job totals — the snappiest possible shape for a
single-purpose view.

*Rejected — pure `/api/stats` (counts only) + reuse `/api/jobs?per_page=8`
+ a new boards-slice endpoint:* more granular and arguably more RESTful,
but it means three round-trips and more UI state for no benefit here, and
it still adds the same boards-slice query.

### 3.4 The "Board fleet" card shows a small, relevant slice

Beyond tallies, the fleet card renders rows. Instead of "every board up
to 100", it shows the **most operationally relevant** N (default 8):
**quarantined boards first, then most-recently-used**. The stat card
still reports the true `healthy/total` from the SQL aggregate, so nothing
is hidden — the slice is just the at-a-glance "what needs attention /
what's active" view, with the full fleet one click away on `/boards`.

*Why:* keeps the dashboard bounded and fast at any fleet size while
surfacing exactly the boards an operator cares about on a landing page.

### 3.5 Counts from SQL, lists from their natural source

Within the one handler: the **counts** come from SQL (exact, all-time);
the **8 recent jobs** come from the existing in-memory `JobIndex`
(newest-first, already in memory — preserves "the jobs list reads the
index, not sqlite"); the **fleet slice** comes from SQL (boards are not
held in memory). The counts and the index are refreshed by the same
poller and the dashboard only refetches on a poller-driven revision bump,
so the brief eventual-consistency between them is unobservable in
practice.

## 4. Architecture

Changes span four crates, following the dependency DAG
(`proto → db → web → web-ui`). No back-edges.

### 4.1 Wire types — `crates/paavo-proto/src/stats.rs` (new module)

Pure data, `deny_unknown_fields`, additive. Re-exported from `lib.rs`.

```rust
/// All-time job counts by state, over retained rows. The dashboard's
/// derived tallies (queue depth, terminal total, pass rate) are computed
/// from these via the helpers below, so "what counts as queued /
/// terminal" has exactly one definition shared by every consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobStateCounts {
    pub submitted: u64,
    pub building: u64,
    pub awaiting_board: u64,
    pub running: u64,
    pub passed: u64,
    pub failed: u64,
    pub timed_out: u64,
    pub aborted: u64,
}

impl JobStateCounts {
    /// Jobs accepted but not yet running: submitted + building + awaiting_board.
    pub fn queue(&self) -> u64 { self.submitted + self.building + self.awaiting_board }
    /// Jobs in a terminal state: passed + failed + timed_out + aborted.
    pub fn terminal(&self) -> u64 { self.passed + self.failed + self.timed_out + self.aborted }
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
    pub total: u64,
    pub quarantined: u64,
}

impl BoardHealthCounts {
    /// total - quarantined (saturating; the two are always consistent in
    /// a single snapshot but saturating keeps the type total-correct).
    pub fn healthy(&self) -> u64 { self.total.saturating_sub(self.quarantined) }
}

/// One-shot payload backing the dashboard landing page: exact aggregate
/// counts plus the two short display lists the page renders. Fully
/// bounded — its size does not grow with the fleet or job history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardOverview {
    pub jobs: JobStateCounts,
    pub boards: BoardHealthCounts,
    /// Newest-first; capped (default 8) for the "Recent activity" table.
    pub recent_jobs: Vec<JobListItem>,
    /// Quarantined-first then most-recently-used; capped (default 8) for
    /// the "Board fleet" table.
    pub fleet: Vec<BoardView>,
    /// Resource revisions at query time, echoed for live de-dup / debug.
    pub jobs_revision: u64,
    pub boards_revision: u64,
}
```

Tests (house style): serde round-trips for each type; `queue`/`terminal`
arithmetic; `pass_rate_pct` rounding and the `terminal == 0 → None` edge;
`healthy` derivation.

### 4.2 DB queries — `crates/paavo-db` (no migration)

`JobRow` (`src/job.rs`):

```rust
/// Job counts grouped by state, all-time over retained rows. Backed by
/// `idx_job_state` (SQLite satisfies GROUP BY from the index). Unknown
/// state strings surface as `DbError::UnknownEnum`, matching `from_row`.
pub fn state_counts(conn: &Connection) -> Result<paavo_proto::JobStateCounts> {
    // SELECT state, COUNT(*) FROM job GROUP BY state
    // fold each (state_from_str, n) into the zeroed struct.
}
```

`BoardRow` (`src/board.rs`):

```rust
/// Total board count and how many are quarantined, in one pass.
pub fn health_counts(conn: &Connection) -> Result<paavo_proto::BoardHealthCounts> {
    // SELECT COUNT(*),
    //        COALESCE(SUM(CASE WHEN health='quarantined' THEN 1 ELSE 0 END), 0)
    //   FROM board
}

/// The N most operationally-relevant boards for the dashboard fleet
/// card: quarantined first, then most-recently-used, "never used" last,
/// `id` as the deterministic tiebreak.
pub fn list_dashboard(conn: &Connection, limit: u32) -> Result<Vec<Self>> {
    // SELECT * FROM board
    // ORDER BY (health = 'quarantined') DESC,  -- 1 before 0
    //          last_used_at DESC,              -- NULLs sort last under DESC
    //          id ASC
    // LIMIT ?1
}
```

Tests via `tempfile` DB seeded with mixed-state jobs and
healthy/quarantined boards: `state_counts` returns the right per-state
tallies (including zero buckets); `health_counts` matches; `list_dashboard`
puts quarantined first, then orders by `last_used_at` desc, places
never-used last, and honours `limit`.

### 4.3 Web endpoint — `crates/paavo-web`

`src/db.rs` (RO façade) gains three thin wrappers:

```rust
pub fn job_state_counts(&self) -> paavo_db::Result<paavo_proto::JobStateCounts>
pub fn board_health_counts(&self) -> paavo_db::Result<paavo_proto::BoardHealthCounts>
pub fn boards_dashboard(&self, limit: u32) -> paavo_db::Result<Vec<paavo_db::BoardRow>>
```

`src/api/dashboard.rs` (new), registered in `api/mod.rs` and `app.rs`:

```rust
/// GET /api/dashboard — one bounded payload for the landing page: exact
/// SQL aggregate counts + the two short display lists. Extracts the whole
/// AppState because it needs the DB (counts + fleet slice), the live
/// index (recent jobs), and the current revisions. No `.await` between
/// taking the index read-guard and dropping it (no lock across await).
pub async fn get(State(s): State<AppState>)
    -> Result<Json<DashboardOverview>, (StatusCode, String)>
{
    let err = |e: paavo_db::DbError| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let jobs   = s.db.job_state_counts().map_err(err)?;
    let boards = s.db.board_health_counts().map_err(err)?;
    let fleet  = s.db.boards_dashboard(FLEET_SLICE).map_err(err)?
                     .into_iter().map(board_view).collect();
    let recent_jobs = {                       // top-N from the in-memory index
        let (items, _) = s.live.index.read().search("", None, 1, RECENT_JOBS);
        items
    };
    let rev = s.live.revisions();
    Ok(Json(DashboardOverview {
        jobs, boards, recent_jobs, fleet,
        jobs_revision: rev.jobs, boards_revision: rev.boards,
    }))
}
```

`RECENT_JOBS` and `FLEET_SLICE` are module consts (both 8). `board_view`
is the same `BoardRow → BoardView` projection used by `api/boards.rs`
(factor out or duplicate the tiny mapper — one source preferred).

`app.rs`: `.route("/api/dashboard", get(crate::api::dashboard::get))`.
Registered before the asset fallback, like the other `/api/*` routes.

Integration test (mirrors `index.rs`/`tests/` harness: RW `Db` seeds a
temp file; RO `WebDb` reads it via WAL; build the router): seed jobs
across several states and a quarantined + healthy board, `GET
/api/dashboard`, assert `200`, that `jobs`/`boards` counts reflect the
seeded rows, that `recent_jobs` is newest-first and length-capped, and
that `fleet` leads with the quarantined board.

### 4.4 Dashboard rewrite — `crates/paavo-web-ui`

`src/api.rs`:

```rust
/// GET /api/dashboard — the consolidated landing-page payload.
pub async fn dashboard() -> Result<DashboardOverview, String> {
    fetch_json("/api/dashboard").await
}
```

`src/components/dashboard.rs`:

- Replace the two `LocalResource`s (200-job + 100-board) with **one**
  keyed on both live signals:
  ```rust
  let over = LocalResource::new(move || {
      let _ = live.jobs.get();
      let _ = live.boards.get();
      async move { api::dashboard().await }
  });
  ```
- `render(over: DashboardOverview)` drops every `.iter().filter().count()`
  and reads the values directly:
  - "Running" ← `over.jobs.running`
  - "Queue" ← `over.jobs.queue()`
  - "Boards" ← `over.boards.healthy()` / `over.boards.total`; subtitle
    `over.boards.quarantined` when `> 0`
  - "Pass rate" (label no longer "(recent)") ← `over.jobs.pass_rate_pct()`
    rendered as `"{n}%"` or `"—"`; subtitle `"{over.jobs.terminal()} runs"`
  - "Recent activity" table ← `over.recent_jobs`
  - "Board fleet" table ← `over.fleet`

  The row-rendering closures for the two tables are unchanged; only their
  input source changes. Net: less code, exact numbers, bounded work.

`paavo-web-ui` is workspace-excluded (wasm32) — it is verified by `just
build-ui` (trunk) and the manual smoke, not `cargo test --workspace`.

## 5. Failure modes & edge cases

- **Empty DB:** all counts zero; `pass_rate_pct()` → `None` → "—";
  `recent_jobs` / `fleet` empty → the existing "no jobs yet" / "no boards
  registered" empty-state rows. Unchanged behaviour.
- **Fleet larger than the slice:** the card shows the 8 most relevant
  boards; the stat card still shows the true `healthy/total`. No
  truncation of the *number*, only of the *list*.
- **Counts vs. recent-jobs momentary skew:** counts (SQL) and recent_jobs
  (in-memory index) are both refreshed by the one poller; the dashboard
  refetches only on a poller-driven revision bump, so they are consistent
  at every observed render.
- **DB read error:** handler maps `DbError` → 500; the SPA shows its
  existing "failed to load dashboard" branch. A transient WAL hiccup
  recovers on the next live-signal refetch.
- **Unknown state/health string in a row:** `DbError::UnknownEnum` → 500,
  same as every other typed query — a schema-invariant violation, not
  user input.

## 6. Testing strategy

1. **paavo-proto** (`stats.rs` `#[cfg(test)]`): serde round-trips;
   `queue`/`terminal` sums; `pass_rate_pct` rounding + `terminal == 0`
   → `None`; `healthy` derivation.
2. **paavo-db** (`#[cfg(test)]`, `tempfile`): `state_counts` per-state
   tallies incl. zero buckets; `health_counts`; `list_dashboard` ordering
   (quarantined first, `last_used_at` desc, never-used last) + `limit`.
3. **paavo-web** (integration, temp DB + router): `GET /api/dashboard`
   returns `200` with counts reflecting seeded rows, newest-first
   length-capped `recent_jobs`, and quarantined-first `fleet`.
4. **paavo-web-ui**: compile-checked via `just build-ui`; covered by the
   manual smoke in §7 (no wasm test harness is stood up).

## 7. Definition of done

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, and `cargo test --workspace` all green.
- `just build-ui` builds the SPA without error.
- Manual smoke (`PAAVO_FAKE_RUNNER=1` daemon + `paavo-web`): open `/`;
  the four stat cards show correct numbers; submitting jobs from another
  terminal updates "Running"/"Queue"/"Pass rate" and "Recent activity"
  live (via the existing revision→refetch path); quarantining a board
  moves it to the top of "Board fleet" and updates the "Boards" card.

## 8. Scope — explicitly out

- **Schedules on the dashboard.** No schedule card exists today; none is
  added. `DashboardOverview` is extensible if one is wanted later.
- **Index/list pages** (`/jobs`, `/boards`, `/schedules`). They already
  show exact totals via `Page.total`; untouched.
- **Time-windowed or "last N" pass rate** (§3.2 rejected).
- **A new DB migration / index.** `idx_job_state` already exists; the
  board table is small enough that its aggregate needs no new index.
- **Changing the `/api/events` SSE contract** or any paavod behaviour.
- **Configurable slice sizes via config.** `RECENT_JOBS` / `FLEET_SLICE`
  are consts (8); promote to `[web]` config only if a deployment needs it.

## 9. Files touched

| File | Change |
| --- | --- |
| `crates/paavo-proto/src/stats.rs` | **New.** `JobStateCounts`, `BoardHealthCounts`, `DashboardOverview` + helper methods + serde/helper tests |
| `crates/paavo-proto/src/lib.rs` | `mod stats;` + `pub use stats::{...}` |
| `crates/paavo-db/src/job.rs` | `JobRow::state_counts` + tests |
| `crates/paavo-db/src/board.rs` | `BoardRow::health_counts`, `BoardRow::list_dashboard` + tests |
| `crates/paavo-web/src/db.rs` | `job_state_counts`, `board_health_counts`, `boards_dashboard` façade wrappers |
| `crates/paavo-web/src/api/dashboard.rs` | **New.** `GET /api/dashboard` handler + `board_view` projection |
| `crates/paavo-web/src/api/mod.rs` | `pub mod dashboard;` |
| `crates/paavo-web/src/app.rs` | `.route("/api/dashboard", ...)` |
| `crates/paavo-web/tests/` | New integration test for `/api/dashboard` |
| `crates/paavo-web-ui/src/api.rs` | `dashboard()` fetch wrapper |
| `crates/paavo-web-ui/src/components/dashboard.rs` | One `LocalResource`; `render` reads aggregates instead of counting |

**paavod:** unchanged. **Migration:** none. **Proto change:** additive
(new module + new endpoint payload). **Estimated size:** ~250–350 LOC
including tests.
