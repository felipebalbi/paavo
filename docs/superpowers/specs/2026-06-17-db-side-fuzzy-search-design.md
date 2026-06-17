# Design: DB-side fuzzy job search (drop the in-memory index)

**Status**: design approved 2026-06-17. `paavo-db` + `paavo-web` +
`paavo-web-ui` change. **No schema migration**, **no proto change**, **no
`WireMessage` / `LogFrame` shape change**, **no `paavod` change**, no change
to the `/api/events` SSE contract. Net dependency move: `fuzzy-matcher`
leaves `paavo-web` and joins `paavo-db`.

---

## 1. Goal

Job fuzzy search currently runs against a **full-history in-memory
projection** (`crate::index::JobIndex` in `paavo-web`): the background poller
loads every job's searchable fields into RAM each tick, and `GET /api/jobs`
fuzzy-matches over that `Vec`. The structure grows without bound as job
history accumulates.

Move fuzzy search — and every other consumer of that index — **into SQLite**,
so the web process no longer holds the whole job history in memory just to
search it. SQLite drives row iteration and returns only the requested page;
the process keeps nothing per-job resident. This is the same "offload the
work to the database, snappy at scale" direction as the
2026-06-17 dashboard-SQL-aggregates design, applied to search.

Searching must stay **true fzf-style**: the query `almcx` must match (and
rank) `alice … mcxa266-01`, exactly as the in-memory `SkimMatcherV2` does
today. A plain substring `LIKE '%almcx%'` would not — hence the design below.

## 2. Background: how search works today

- **`crates/paavo-web/src/index.rs`** defines `JobIndex { items:
  Vec<JobListItem>, haystacks: Vec<String> }`. `haystack()` builds a
  lowercased `"{id} {submitter} {state:?} {board_id}"` per row. `search()`
  runs `SkimMatcherV2::fuzzy_match` over every haystack, sorts by score
  (newest-first tiebreak), then paginates. `new_count()` counts rows newer
  than the `as_of` pin (the "↑ N new" pill).
- **The poller** (`spawn_poller`) rebuilds `JobIndex` from
  `db.jobs_index()` → `JobRow::list_index` (a lightweight all-rows
  projection) whenever a content fingerprint changes, and bumps a per-resource
  revision that fans out over `/api/events`. The jobs fingerprint `fp_jobs`
  hashes `id + state` of **every** job.
- **`crates/paavo-web/src/api/jobs.rs::list`** reads `LiveState.index` for
  both blank-query list mode (time-ordered, `as_of`-pinned) and non-blank
  search mode (fuzzy-ranked, `new_count` forced to 0).
- **`crates/paavo-web-ui/src/components/jobs_list.rs`** debounces keystrokes
  by **150 ms** (generation-counter + `gloo_timers::Timeout`) before
  committing the query the data resource is keyed on.

The job table (`migrations/V1__initial.sql`) has indexes `idx_job_state`,
`idx_job_submitted_at`, and a partial `idx_job_priority_subat`. No
full-text/trigram index exists.

## 3. Decisions

### 3.1 Split membership from ranking: subsequence `LIKE` + a `fuzzy_score` UDF

Two SQL concerns, two mechanisms:

- **Membership** ("does this job match?") → a **subsequence `LIKE`**. The
  query `almcx` becomes the pattern `%a%l%m%c%x%` (each character escaped for
  `%`/`_`/`\`, joined and bracketed by `%`). `LIKE '%a%l%m%c%x%'` is true iff
  those characters appear *in order* — precisely the subsequence test, so
  `almcx` matches `alice … mcxa266`. This is a plain SQL string comparison: no
  UDF call.
- **Ranking** ("how good is the match?") → a custom SQLite scalar function
  **`fuzzy_score(haystack, query) -> Option<i64>`** backed by the same
  `SkimMatcherV2` used today, invoked **only in `ORDER BY`**, **only on the
  rows `LIKE` already kept**.

*Chosen because* it keeps ranking byte-for-byte identical to today while
calling the expensive cross-boundary UDF on the *matched subset only* — and
the pagination `COUNT(*)` calls it **zero** times (pure `LIKE`). SQLite
materializes only the requested page, meeting the memory goal.

*Rejected — UDF in the `WHERE` clause (no `LIKE` pre-filter):* simpler SQL,
but the UDF then fires on **every** row, twice per keystroke (page + count).
The `LIKE` pre-filter reduces that to a cheap per-row string compare plus a
UDF call only on matches.

*Rejected — substring `LIKE '%almcx%'`:* simplest, but substring ≠
subsequence; `almcx` would not match `alice … mcxa266`. Fails the core
requirement.

*Rejected — FTS5:* token/prefix BM25 search has different (non-subsequence)
semantics and needs a migration plus write-path triggers kept in sync with
`paavod`. Heavier; out of scope.

### 3.2 Eliminate `JobIndex`; every consumer reads SQL

Keeping the index in RAM for the live pill or the dashboard would defeat the
goal (the full projection would still be resident). So `JobIndex` is deleted
and `LiveState` drops its `index` field. Each consumer moves to a typed
`paavo-db` query (§4.2):

| Consumer | Today | After |
|---|---|---|
| `/api/jobs` search (non-blank `q`) | `index.search(q,…)` | `JobRow::search_index_page` + `search_count` |
| `/api/jobs` list (blank `q`, `as_of`-pinned) | `index.search("",…)` | `JobRow::list_index_page` + `JobRow::count` |
| "↑ N new" pill | `index.new_count(as_of)` | `JobRow::count_newer(as_of)` |
| Poller change-detection | hash `id+state` of every job | `JobRow::activity_digest()` (bounded aggregate) |

### 3.3 Poller change-detection becomes a bounded aggregate fingerprint

Today's `fp_jobs` is *exact* — any job's state change anywhere bumps the jobs
revision — but it materializes the whole table each tick. It is replaced by a
single index-backed aggregate:

```sql
SELECT state, COUNT(*), COALESCE(MAX(submitted_at), 0) FROM job GROUP BY state
```

hashed into a `u64` digest (≤ 8 rows). This catches **every insert** (a
group's count and/or max moves) and **every state transition** (counts shift
between groups). It is a *heuristic*: it misses only the rare case where two
jobs make offsetting transitions within one poll interval such that all
per-state counts and maxima land identical — which self-heals on the next
tick, and is only a "should the client refetch" hint, never a correctness
input. Boards/schedules fingerprints are unchanged (small, id-stable tables).

*Accepted trade-off, signed off during design review.*

### 3.4 Case handling — lowercase both sides

The haystack is lowercased in SQL (`lower(...)`) so the `LIKE` operand and the
UDF input are byte-identical; the query is lowercased in Rust before both the
`LIKE` pattern and the UDF argument are built. With an all-lowercase query,
`SkimMatcherV2`'s smart-case reduces to plain case-insensitive matching, so
the `LIKE` candidate set and the `fuzzy_score`-returns-`Some` set coincide on
the ASCII content these haystacks carry (ULID ids, ASCII states, board ids,
usernames). A unit test asserts this equivalence (§6). This also *improves*
on today's behavior, where the haystack was lowercased but the raw query was
not (so an uppercase keystroke mostly failed to match).

**Graceful degradation:** even if the two sets ever diverged, a row that
`LIKE`-matches but the matcher scores `NULL` simply sorts **last** under
`ORDER BY score DESC` (SQLite sorts `NULL` last under `DESC`) and is still a
legitimate subsequence match — a benign ordering anomaly, not a correctness
bug. The safety valve, if ever needed, is to `AND fuzzy_score(...) IS NOT
NULL` into the `WHERE`.

### 3.5 Debounce 150 → 250 ms

With search now a **table scan** per committed keystroke (rather than an
in-memory match), more typing quiet is warranted. `jobs_list.rs` already
debounces via a generation counter; this is a one-line retune (`Timeout::new(
150, …)` → `250`) plus the doc-comment update. The mechanism is unchanged.

## 4. Architecture

Changes follow the dependency DAG (`proto → db → web → web-ui`); no
back-edges. `paavo-proto` is untouched (`JobListItem` / `Page<JobListItem>`
are reused verbatim).

### 4.1 The `fuzzy_score` function — `crates/paavo-db/src/db.rs`

Registered in `configure()` (runs for both RW and RO connections; `paavod`
simply never calls it):

```rust
// Build the matcher once and capture it; the function is called per
// matched row, so re-constructing SkimMatcherV2 (and its scoring tables)
// each call would be wasteful. SkimMatcherV2 is Send + Sync (config only,
// no interior mutability), satisfying the closure's bounds.
// SkimMatcherV2 is deterministic for fixed inputs; flag the function so
// SQLite can cache/factor calls. create_scalar_function is a safe API —
// no `unsafe`, so paavo-db keeps #![forbid(unsafe_code)].
let matcher = SkimMatcherV2::default();
conn.create_scalar_function(
    "fuzzy_score",
    2,
    FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
    move |ctx| {
        // get_raw().as_str() borrows the column bytes — no per-row String
        // allocation. Arg 0 is lower()ed in SQL; arg 1 is lowercased in Rust.
        let haystack = ctx.get_raw(0).as_str()?;
        let needle = ctx.get_raw(1).as_str()?;
        Ok(matcher.fuzzy_match(haystack, needle)) // Option<i64>; None => SQL NULL
    },
)?;
```

`fuzzy-matcher = "0.3"` is added to `paavo-db`'s `Cargo.toml`. (If
`SkimMatcherV2` should turn out not to be `Sync` on the pinned version, fall
back to constructing it inside the closure — a measured, not assumed,
decision left to implementation.)

### 4.2 Queries — `crates/paavo-db/src/job.rs` (no migration)

The lowercased haystack expression, written once as a SQL fragment:

```sql
lower(id || ' ' || submitter || ' ' || state || ' ' || coalesce(board_id, ''))
```

**Search page** (non-blank query) — UDF only on matched rows, only for rank:

```sql
SELECT id, state, priority, submitter, board_id, submitted_at,
       fuzzy_score(<haystack>, ?1) AS score          -- ?1 = lowercased query
FROM job
WHERE <haystack> LIKE ?2 ESCAPE '\'                  -- ?2 = %a%l%m%c%x%
ORDER BY score DESC, submitted_at DESC, id DESC
LIMIT ?3 OFFSET ?4;
```

**Search count** (pagination total) — pure `LIKE`, no UDF:

```sql
SELECT COUNT(*) FROM job WHERE <haystack> LIKE ?1 ESCAPE '\';
```

**List page** (blank query, `as_of`-pinned, lightweight projection):

```sql
SELECT id, state, priority, submitter, board_id, submitted_at
FROM job
WHERE (?1 IS NULL OR submitted_at <= ?1)
ORDER BY submitted_at DESC, id DESC
LIMIT ?2 OFFSET ?3;
```

**New-count** (the pill): `SELECT COUNT(*) FROM job WHERE submitted_at > ?1`.

**Activity digest** (poller): `SELECT state, COUNT(*), COALESCE(MAX(
submitted_at), 0) FROM job GROUP BY state`, folded into a `u64` hash.

New public methods on `JobRow`, each returning the lightweight
`paavo_proto::JobListItem` projection where rows are returned:
`search_index_page`, `search_count`, `list_index_page`, `count_newer`,
`activity_digest`. The existing `count(conn, as_of)` is reused for list-mode
totals. The now-unused `list_index` is removed.

A `pub(crate)` LIKE helper module holds `escape_like` (moved from `board.rs`,
which currently has a private copy) and `subsequence_pattern(query) ->
String`, so wildcard escaping lives in one place.

### 4.3 RO façade + poller + handler — `crates/paavo-web`

- **`src/db.rs`** (`WebDb`): add thin wrappers `jobs_search_page`,
  `jobs_search_count`, `jobs_list_page`, `jobs_new_count`, and
  `jobs_activity_digest`; remove `jobs_index`. Each is one short
  lock/query/unlock on the RO connection — no lock is held across an `.await`
  (the handler has no await between calls), consistent with the existing
  `db.rs` "sync calls in async handlers" rationale.
- **`src/index.rs`**: delete `JobIndex` and its tests; `LiveState` loses the
  `index` field and its `Arc<RwLock<JobIndex>>`. `spawn_poller` replaces the
  `db.jobs_index()` → rebuild step with `db.jobs_activity_digest()` →
  fingerprint compare. `Revisions`, the watch channel, and the boards/schedule
  fingerprints are unchanged.
- **`src/api/jobs.rs::list`**: read rows + totals from `WebDb`; branch on
  blank vs non-blank `q` exactly as today (blank → list page + `count` +
  `count_newer`; non-blank → search page + `search_count`, `new_count = 0`).
  The `revision` echoed in `Page` still comes from `LiveState::revisions()`.

### 4.4 Debounce — `crates/paavo-web-ui/src/components/jobs_list.rs`

`Timeout::new(150, …)` → `Timeout::new(250, …)`; update the "Debounce"
doc-comment's "150 ms" to "250 ms". No other change. `paavo-web-ui` is
workspace-excluded (wasm32) — verified by `just build-ui` + manual smoke, not
`cargo test --workspace`.

## 5. Failure modes & edge cases

- **Blank / whitespace query:** list mode (no `LIKE`, no UDF); `as_of` pin and
  the pill behave exactly as today.
- **Query of all wildcard chars (`%`, `_`):** escaped via `ESCAPE '\'`, so
  they match literally rather than acting as wildcards.
- **No matches:** `search_count` → 0, `search_index_page` → empty; the SPA
  renders its existing "0 matches" state.
- **`LIKE`/matcher divergence:** degrades gracefully (§3.4) — a `LIKE`-only
  row sorts last with a `NULL` score; never a wrong-row or crash.
- **DB read error:** handler maps `DbError` → 500; the SPA shows its existing
  "failed to load jobs" branch; a transient WAL hiccup recovers on the next
  live-signal refetch.
- **Poller fingerprint blind spot:** offsetting same-tick transitions (§3.3)
  may delay a revision bump by one interval; self-heals.
- **Unknown state string in a row:** surfaces as `DbError::UnknownEnum` from
  the row decoder, same as every other typed query.

## 6. Testing strategy

House style — hand-written assertions, `tempfile` DBs, no new test deps.

1. **paavo-db** (`#[cfg(test)]`, tempfile): register `fuzzy_score`, then
   - `almcx` matches `alice … mcxa266` and ranks the best match first;
   - `search_index_page` pagination (page/size slicing) and `search_count`
     parity with the matched total;
   - **divergence guard:** on a mixed fixture, the set of rows passing the
     `LIKE` pattern equals the set for which `SkimMatcherV2` returns `Some`;
   - `list_index_page` ordering (`submitted_at DESC, id DESC`), the `as_of`
     pin, and paging;
   - `count_newer` strict-`>` boundary;
   - `activity_digest` changes on insert and on a state transition, and is
     stable across an unrelated no-op.
2. **paavo-web** (integration, RW `Db` seeds a temp file, RO `WebDb` reads via
   WAL, build the router): the existing `tests/api_jobs.rs` (pagination +
   `q=alice` narrowing) now exercises the SQL path end-to-end and still
   passes; the `index.rs` poller test keeps its "jobs revision bumps after
   insert" assertion, with the index-read removed.
3. **paavo-web-ui**: compile-checked via `just build-ui`; the 250 ms debounce
   and live ranking are covered by the manual smoke (§7).

## 7. Definition of done

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, and `cargo test --workspace` all green.
- `just build-ui` builds the SPA without error.
- Manual smoke (`PAAVO_FAKE_RUNNER=1` daemon + `paavo-web`): open `/jobs`;
  typing `almcx` ranks `alice … mcxa266` first; clearing the box returns to
  the time-ordered list; submitting jobs from another terminal updates the
  list and the "↑ N new" pill live; the query only fires after ~250 ms of
  typing quiet.

## 8. Scope — explicitly out

- **A migration, FTS5, or a trigram/generated-column index.** The haystack is
  computed inline; search is an honest O(n) scan per committed keystroke,
  which is the accepted trade for bounded memory at lab scale. Revisit only if
  the job table reaches a scale where a per-keystroke scan lags.
- **`paavod` / writer-side changes**, the `/api/events` SSE contract, and any
  `JobListItem` / `Page` wire-shape change.
- **Boards/schedules search or fingerprints** (small, id-stable; untouched).
- **Per-job log search** — already a separate client-side filter.

## 9. Cross-branch coordination

The in-flight `feat/dashboard-sql-aggregates` branch's design (§3.5) sources
the dashboard's "Recent activity" list from `live.index.read().search("",
…)`. Deleting `JobIndex` removes that read. The clean resolution — which
*strengthens* that branch's own "counts from SQL, snappy at scale" thesis — is
for its recent-jobs list to come from `JobRow::list_index_page(None, 0, 8)`
instead. Whichever branch lands second should adopt the SQL source rather than
reintroduce the index.

## 10. Files touched

| File | Change |
| --- | --- |
| `crates/paavo-db/Cargo.toml` | add `fuzzy-matcher = "0.3"` |
| `crates/paavo-db/src/db.rs` | register `fuzzy_score` UDF in `configure` |
| `crates/paavo-db/src/job.rs` | `search_index_page`, `search_count`, `list_index_page`, `count_newer`, `activity_digest`; remove `list_index` |
| `crates/paavo-db/src/like.rs` *(new)* | `pub(crate) escape_like` + `subsequence_pattern` |
| `crates/paavo-db/src/board.rs` | use the shared `escape_like` (drop the private copy) |
| `crates/paavo-web/Cargo.toml` | remove `fuzzy-matcher` (no longer used) |
| `crates/paavo-web/src/index.rs` | delete `JobIndex`; `LiveState` drops `index`; poller uses `activity_digest` |
| `crates/paavo-web/src/db.rs` | add search/list/new-count/digest wrappers; remove `jobs_index` |
| `crates/paavo-web/src/api/jobs.rs` | `list` reads SQL instead of the index |
| `crates/paavo-web/tests/api_jobs.rs` | exercises the SQL path (assertions preserved) |
| `crates/paavo-web-ui/src/components/jobs_list.rs` | debounce 150 → 250 ms + doc comment |

**Migration:** none. **Proto change:** none. **Estimated size:** ~250–350 LOC
including tests.
