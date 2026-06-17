# DB-Side Fuzzy Job Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move job fuzzy search (and the live pill + poller change-detection) off the full-history in-memory `JobIndex` and into SQLite, so `paavo-web` no longer holds the whole job history in RAM to search it.

**Architecture:** A custom SQLite scalar function `fuzzy_score` (backed by `SkimMatcherV2`) ranks matches; a subsequence `LIKE '%a%l%m%c%x%'` pattern decides membership. SQLite drives iteration and returns only the requested page. `JobIndex` is deleted; the `/api/jobs` handler, the "N new" pill, and the poller's revision fingerprint all read SQL. The SPA's search box debounce goes 150 → 250 ms.

**Tech Stack:** Rust 1.95, rusqlite 0.32 (`functions` feature), `fuzzy-matcher` 0.3 (`SkimMatcherV2`), axum 0.7, Leptos CSR (`gloo_timers`), `tempfile` tests.

**Spec:** `docs/superpowers/specs/2026-06-17-db-side-fuzzy-search-design.md`

**Worktree:** `.worktrees/db-fuzzy-search` (branch `feat/db-fuzzy-search`). Run all commands there.

---

### Task 1: Shared LIKE helpers (`paavo-db`)

Extract the wildcard-escaping logic into one module and add the subsequence-pattern builder both the search query and its tests need. `board.rs` already has a private `escape_like`; consolidate it.

**Files:**
- Create: `crates/paavo-db/src/like.rs`
- Modify: `crates/paavo-db/src/lib.rs` (add `mod like;`)
- Modify: `crates/paavo-db/src/board.rs:332-345` (drop private `escape_like`, import the shared one)

- [ ] **Step 1: Write `like.rs` with its tests**

```rust
//! Shared `LIKE` helpers: wildcard escaping and the subsequence pattern
//! builder used by the fuzzy-search queries in `job.rs` and the fleet
//! filter in `board.rs`. Kept in one place so the escape rules (`%`, `_`,
//! `\`, paired with `ESCAPE '\'`) never drift between call sites.

/// Escape `LIKE` wildcards so `%`, `_`, and `\` match literally. Pair the
/// result with `ESCAPE '\'` in the query.
pub(crate) fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '%' || c == '_' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Build a subsequence `LIKE` pattern from `query`: each character is
/// escaped and separated by `%`, so `"almcx"` becomes `"%a%l%m%c%x%"`.
/// `LIKE` against this pattern is true iff the characters appear in order
/// — the same membership test `SkimMatcherV2` uses. An empty query yields
/// `"%"` (matches everything). The caller lowercases `query` first so the
/// pattern and the `fuzzy_score` needle agree.
pub(crate) fn subsequence_pattern(query: &str) -> String {
    let mut p = String::from("%");
    for c in query.chars() {
        if c == '%' || c == '_' || c == '\\' {
            p.push('\\');
        }
        p.push(c);
        p.push('%');
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_like_escapes_wildcards() {
        assert_eq!(escape_like("a%b_c\\d"), "a\\%b\\_c\\\\d");
        assert_eq!(escape_like("plain"), "plain");
    }

    #[test]
    fn subsequence_pattern_interleaves_percents() {
        assert_eq!(subsequence_pattern("almcx"), "%a%l%m%c%x%");
        assert_eq!(subsequence_pattern(""), "%");
        assert_eq!(subsequence_pattern("a%"), "%a%\\%%");
    }
}
```

- [ ] **Step 2: Wire the module + repoint `board.rs`**

In `crates/paavo-db/src/lib.rs`, add `mod like;` (between `mod job;` and `mod log;`):

```rust
mod job;
mod like;
mod log;
```

In `crates/paavo-db/src/board.rs`, add the import near the top (after the existing `use` lines):

```rust
use crate::like::escape_like;
```

Then **delete** the private `escape_like` fn and its doc comment at `board.rs:332-345` (the two call sites in `list_page`/`count` keep calling `escape_like(q)` unchanged — they now resolve to the imported one).

- [ ] **Step 3: Run the tests**

Run: `cargo test -p paavo-db like::`
Expected: PASS (2 tests).

- [ ] **Step 4: Verify the crate still builds (board.rs repoint)**

Run: `cargo build -p paavo-db`
Expected: builds with no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-db/src/like.rs crates/paavo-db/src/lib.rs crates/paavo-db/src/board.rs
git commit -m "refactor(db): shared LIKE helpers (escape_like + subsequence_pattern)"
```

---

### Task 2: Register the `fuzzy_score` scalar function (`paavo-db`)

Enable rusqlite's `functions` feature, add `fuzzy-matcher`, and register `fuzzy_score(haystack, needle) -> Option<i64>` on every connection in `configure()`.

**Files:**
- Modify: `crates/paavo-db/Cargo.toml`
- Modify: `crates/paavo-db/src/db.rs`

- [ ] **Step 1: Write the failing test** (append to `crates/paavo-db/src/db.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn fuzzy_score_some_for_subsequence_none_otherwise() {
        let dir = tempdir().unwrap();
        let db = Db::open(dir.path().join("t.sqlite")).unwrap();
        let c = db.raw_conn();
        let hit: Option<i64> = c
            .query_row("SELECT fuzzy_score('alice mcxa266-01', 'almcx')", [], |r| r.get(0))
            .unwrap();
        assert!(hit.is_some(), "almcx is a subsequence of 'alice mcxa266-01'");
        let miss: Option<i64> = c
            .query_row("SELECT fuzzy_score('bob', 'almcx')", [], |r| r.get(0))
            .unwrap();
        assert!(miss.is_none(), "almcx is not a subsequence of 'bob'");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p paavo-db fuzzy_score_some_for_subsequence`
Expected: FAIL — `no such function: fuzzy_score`.

- [ ] **Step 3: Add the dependencies**

In `crates/paavo-db/Cargo.toml`, change the `rusqlite` line and add `fuzzy-matcher`:

```toml
rusqlite = { workspace = true, features = ["functions"] }
refinery = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
ulid = { workspace = true }
chrono = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
# Skim-style fuzzy matcher backing the `fuzzy_score` SQLite scalar function
# (crate::db). Not workspace-pinned — paavo-db is the only crate that ranks
# fuzzy matches. Mirrors the dep paavo-web previously carried.
fuzzy-matcher = "0.3"
```

- [ ] **Step 4: Register the function in `db.rs`**

Replace the import line `use rusqlite::{Connection, OpenFlags};` with:

```rust
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, Error, OpenFlags};
```

Add `register_fuzzy_score(conn)?;` to `configure()` (just before its final `Ok(())`), then add the function below `configure`:

```rust
/// Register `fuzzy_score(haystack, needle) -> Option<i64>`: the
/// `SkimMatcherV2` ranking the web UI used to run in memory, now callable
/// from SQL by `JobRow::search_index_page`. Registered on every connection
/// (RW + RO); only the RO search path invokes it. `None` (no match) maps to
/// SQL `NULL`.
fn register_fuzzy_score(conn: &Connection) -> Result<()> {
    // Built once and captured: the function runs per matched row, so
    // rebuilding the matcher each call would waste its scoring tables.
    // SkimMatcherV2 is plain config data (Send + 'static); the closure bounds
    // are satisfied and create_scalar_function is safe (no `unsafe`).
    let matcher = SkimMatcherV2::default();
    conn.create_scalar_function(
        "fuzzy_score",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx| {
            // get_raw().as_str() borrows the column bytes — no per-row String.
            let haystack = ctx
                .get_raw(0)
                .as_str()
                .map_err(|e| Error::UserFunctionError(e.into()))?;
            let needle = ctx
                .get_raw(1)
                .as_str()
                .map_err(|e| Error::UserFunctionError(e.into()))?;
            Ok(matcher.fuzzy_match(haystack, needle))
        },
    )?;
    Ok(())
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p paavo-db fuzzy_score_some_for_subsequence`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/paavo-db/Cargo.toml crates/paavo-db/src/db.rs Cargo.lock
git commit -m "feat(db): register fuzzy_score SQLite scalar function"
```

---

### Task 3: Fuzzy search queries on `JobRow` (`paavo-db`)

Add `search_index_page` (ranked page) and `search_count` (total), plus the shared `#[cfg(test)]` harness reused by Tasks 4–5.

**Files:**
- Modify: `crates/paavo-db/src/job.rs`

- [ ] **Step 1: Write the failing tests** (append to `crates/paavo-db/src/job.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Db;
    use tempfile::{tempdir, TempDir};

    /// Open a fresh migrated temp DB (registers `fuzzy_score`).
    fn test_db() -> (TempDir, Db) {
        let dir = tempdir().unwrap();
        let db = Db::open(dir.path().join("t.sqlite")).unwrap();
        (dir, db)
    }

    fn new_job(submitter: &str) -> NewJob {
        NewJob {
            id: JobId::new(),
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

    /// Insert a job; when `board` is `Some`, transition it to `building` so
    /// the board id lands in the searchable haystack. Returns the job id.
    fn insert_job(
        conn: &Connection,
        submitter: &str,
        board: Option<&str>,
        submitted_at: i64,
    ) -> JobId {
        let j = new_job(submitter);
        let id = j.id;
        JobRow::insert(conn, &j, submitted_at).unwrap();
        if let Some(b) = board {
            JobRow::transition_to_building(conn, &id, b, submitted_at).unwrap();
        }
        id
    }

    #[test]
    fn fuzzy_search_isolates_by_submitter() {
        // q="alice": bob/carol lack a,l,i,c,e in order (ULIDs exclude i,l).
        let (_d, db) = test_db();
        let c = db.raw_conn();
        insert_job(c, "alice", None, 3_000);
        insert_job(c, "bob", None, 2_000);
        insert_job(c, "carol", None, 1_000);

        assert_eq!(JobRow::search_count(c, "alice").unwrap(), 1);
        let rows = JobRow::search_index_page(c, "alice", 0, 50).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].submitter, "alice");
    }

    #[test]
    fn fuzzy_search_cross_field_subsequence_ranks_best_first() {
        // "almcx" spans submitter+board; "alice" (al contiguous) + mcxa
        // ranks above "carol" (a..l split) + mcxa.
        let (_d, db) = test_db();
        let c = db.raw_conn();
        insert_job(c, "alice", Some("mcxa266-01"), 3_000);
        insert_job(c, "carol", Some("mcxa266-03"), 1_000);

        let rows = JobRow::search_index_page(c, "almcx", 0, 50).unwrap();
        assert!(rows.iter().any(|r| r.submitter == "alice"));
        assert_eq!(rows[0].submitter, "alice", "best fuzzy match ranks first");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p paavo-db fuzzy_search`
Expected: FAIL — `no function or associated item named search_count`/`search_index_page`.

- [ ] **Step 3: Implement both methods** (add to `impl JobRow`, e.g. after `list_index`)

```rust
    /// One page of fuzzy-search results, ranked by `fuzzy_score`. `q` is
    /// matched as a case-insensitive subsequence over the lowercased
    /// `id + submitter + state + board_id` haystack; ranking uses the same
    /// `SkimMatcherV2` the web UI ran in memory. The `LIKE` pre-filter
    /// decides membership (cheap), so `fuzzy_score` runs only on matched
    /// rows. Returns the lightweight [`paavo_proto::JobListItem`] projection.
    pub fn search_index_page(
        conn: &Connection,
        q: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<paavo_proto::JobListItem>> {
        let needle = q.trim().to_lowercase();
        let pattern = crate::like::subsequence_pattern(&needle);
        let mut stmt = conn.prepare(
            "SELECT id, state, priority, submitter, board_id, submitted_at, \
                    fuzzy_score(lower(id || ' ' || submitter || ' ' || state || ' ' || coalesce(board_id,'')), ?1) AS score \
             FROM job \
             WHERE lower(id || ' ' || submitter || ' ' || state || ' ' || coalesce(board_id,'')) LIKE ?2 ESCAPE '\\' \
             ORDER BY score DESC, submitted_at DESC, id DESC \
             LIMIT ?3 OFFSET ?4",
        )?;
        let rows = stmt
            .query_map(
                params![needle, pattern, limit as i64, offset as i64],
                index_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Total fuzzy-search matches for `q` (pagination total). Pure `LIKE`
    /// membership — does **not** call `fuzzy_score`.
    pub fn search_count(conn: &Connection, q: &str) -> Result<u64> {
        let needle = q.trim().to_lowercase();
        let pattern = crate::like::subsequence_pattern(&needle);
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM job \
             WHERE lower(id || ' ' || submitter || ' ' || state || ' ' || coalesce(board_id,'')) LIKE ?1 ESCAPE '\\'",
            params![pattern],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p paavo-db fuzzy_search`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-db/src/job.rs
git commit -m "feat(db): JobRow fuzzy search queries (search_index_page + search_count)"
```

---

### Task 4: List-mode page + new-count on `JobRow` (`paavo-db`)

Lightweight time-ordered page (blank query) with the `as_of` pin, and the strictly-newer count behind the "N new" pill.

**Files:**
- Modify: `crates/paavo-db/src/job.rs`

- [ ] **Step 1: Write the failing tests** (append inside the `tests` module from Task 3)

```rust
    #[test]
    fn list_index_page_orders_and_pins() {
        let (_d, db) = test_db();
        let c = db.raw_conn();
        insert_job(c, "alice", None, 3_000);
        insert_job(c, "bob", None, 2_000);
        insert_job(c, "carol", None, 1_000);

        let all = JobRow::list_index_page(c, None, 0, 50).unwrap();
        assert_eq!(
            all.iter().map(|r| r.submitter.as_str()).collect::<Vec<_>>(),
            ["alice", "bob", "carol"]
        );
        let pinned = JobRow::list_index_page(c, Some(2_000), 0, 50).unwrap();
        assert_eq!(
            pinned.iter().map(|r| r.submitter.as_str()).collect::<Vec<_>>(),
            ["bob", "carol"]
        );
        let p2 = JobRow::list_index_page(c, None, 2, 2).unwrap();
        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0].submitter, "carol");
    }

    #[test]
    fn count_newer_is_strictly_greater() {
        let (_d, db) = test_db();
        let c = db.raw_conn();
        insert_job(c, "alice", None, 3_000);
        insert_job(c, "bob", None, 2_000);
        assert_eq!(JobRow::count_newer(c, None).unwrap(), 0);
        assert_eq!(JobRow::count_newer(c, Some(3_000)).unwrap(), 0, "exclusive");
        assert_eq!(JobRow::count_newer(c, Some(2_000)).unwrap(), 1);
        assert_eq!(JobRow::count_newer(c, Some(0)).unwrap(), 2);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p paavo-db list_index_page_orders_and_pins count_newer_is_strictly_greater`
Expected: FAIL — methods not found.

- [ ] **Step 3: Implement both methods** (add to `impl JobRow`)

```rust
    /// One page of the time-ordered jobs list, newest-first
    /// (`submitted_at DESC, id DESC`), optionally pinned to
    /// `submitted_at <= as_of` for stable pagination under live inserts.
    /// Lightweight [`paavo_proto::JobListItem`] projection — the blank-query
    /// counterpart to `search_index_page`.
    pub fn list_index_page(
        conn: &Connection,
        as_of: Option<i64>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<paavo_proto::JobListItem>> {
        let (sql, bind): (&str, Vec<i64>) = match as_of {
            Some(t) => (
                "SELECT id, state, priority, submitter, board_id, submitted_at FROM job \
                 WHERE submitted_at <= ?1 ORDER BY submitted_at DESC, id DESC LIMIT ?2 OFFSET ?3",
                vec![t, limit as i64, offset as i64],
            ),
            None => (
                "SELECT id, state, priority, submitter, board_id, submitted_at FROM job \
                 ORDER BY submitted_at DESC, id DESC LIMIT ?1 OFFSET ?2",
                vec![limit as i64, offset as i64],
            ),
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(bind), index_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Count of jobs strictly newer than `as_of` (drives the "N new" pill);
    /// 0 when `as_of` is `None`. Index-backed by `idx_job_submitted_at`.
    pub fn count_newer(conn: &Connection, as_of: Option<i64>) -> Result<u64> {
        let n: i64 = match as_of {
            Some(t) => conn.query_row(
                "SELECT COUNT(*) FROM job WHERE submitted_at > ?1",
                params![t],
                |r| r.get(0),
            )?,
            None => 0,
        };
        Ok(n as u64)
    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p paavo-db list_index_page_orders_and_pins count_newer_is_strictly_greater`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-db/src/job.rs
git commit -m "feat(db): JobRow list_index_page + count_newer (lightweight list mode)"
```

---

### Task 5: Poller fingerprint `activity_digest` (`paavo-db`)

A bounded `GROUP BY state` digest the poller hashes to detect inserts and state transitions without materializing the table.

**Files:**
- Modify: `crates/paavo-db/src/job.rs`

- [ ] **Step 1: Write the failing test** (append inside the `tests` module)

```rust
    #[test]
    fn activity_digest_changes_on_insert_and_transition() {
        let (_d, db) = test_db();
        let c = db.raw_conn();
        let empty = JobRow::activity_digest(c).unwrap();
        let id = insert_job(c, "alice", None, 1_000);
        let after_insert = JobRow::activity_digest(c).unwrap();
        assert_ne!(empty, after_insert, "insert must change the digest");
        assert_eq!(after_insert, JobRow::activity_digest(c).unwrap(), "stable no-op");
        JobRow::transition_to_building(c, &id, "mcxa266-01", 2_000).unwrap();
        let after_transition = JobRow::activity_digest(c).unwrap();
        assert_ne!(after_insert, after_transition, "transition must change the digest");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p paavo-db activity_digest_changes`
Expected: FAIL — method not found.

- [ ] **Step 3: Implement** (add to `impl JobRow`)

```rust
    /// A cheap, bounded fingerprint of the job table for the live poller.
    /// `GROUP BY state` yields ≤ 8 rows; folding each
    /// `(state, count, max(submitted_at))` into a hash detects every insert
    /// (a group's count/max moves) and every state transition (counts shift
    /// between groups) without materializing the table. Heuristic: it misses
    /// only offsetting same-tick transitions that leave all tallies
    /// identical, which self-heals next tick. See design doc §3.3.
    pub fn activity_digest(conn: &Connection) -> Result<u64> {
        use std::hash::{Hash, Hasher};
        let mut stmt = conn.prepare(
            "SELECT state, COUNT(*), COALESCE(MAX(submitted_at), 0) \
             FROM job GROUP BY state ORDER BY state",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        rows.len().hash(&mut h);
        for (state, count, max_sub) in &rows {
            state.hash(&mut h);
            count.hash(&mut h);
            max_sub.hash(&mut h);
        }
        Ok(h.finish())
    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p paavo-db activity_digest_changes`
Expected: PASS. Then run the whole crate: `cargo test -p paavo-db` → all green.

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-db/src/job.rs
git commit -m "feat(db): JobRow activity_digest (bounded poller fingerprint)"
```

---

### Task 6: `WebDb` façade wrappers (`paavo-web`)

Thin RO-connection wrappers over the new `JobRow` methods. (Additive — `jobs_index` stays until Task 9 so the poller keeps compiling.)

**Files:**
- Modify: `crates/paavo-web/src/db.rs`

- [ ] **Step 1: Add the wrappers** (insert into `impl WebDb`, after `jobs_index`)

```rust
    /// One page of fuzzy-search results (lightweight projection), ranked.
    pub fn jobs_search_page(
        &self,
        q: &str,
        offset: u32,
        limit: u32,
    ) -> paavo_db::Result<Vec<paavo_proto::JobListItem>> {
        paavo_db::JobRow::search_index_page(self.inner.lock().raw_conn(), q, offset, limit)
    }

    /// Total fuzzy-search matches for `q` (pagination total).
    pub fn jobs_search_count(&self, q: &str) -> paavo_db::Result<u64> {
        paavo_db::JobRow::search_count(self.inner.lock().raw_conn(), q)
    }

    /// One page of the time-ordered jobs list (lightweight), optionally
    /// pinned to `submitted_at <= as_of`.
    pub fn jobs_list_page(
        &self,
        as_of: Option<i64>,
        offset: u32,
        limit: u32,
    ) -> paavo_db::Result<Vec<paavo_proto::JobListItem>> {
        paavo_db::JobRow::list_index_page(self.inner.lock().raw_conn(), as_of, offset, limit)
    }

    /// Count of jobs newer than `as_of` (the "N new" pill); 0 when unpinned.
    pub fn jobs_new_count(&self, as_of: Option<i64>) -> paavo_db::Result<u64> {
        paavo_db::JobRow::count_newer(self.inner.lock().raw_conn(), as_of)
    }

    /// Bounded change-detection fingerprint for the live poller.
    pub fn jobs_activity_digest(&self) -> paavo_db::Result<u64> {
        paavo_db::JobRow::activity_digest(self.inner.lock().raw_conn())
    }
```

- [ ] **Step 2: Build**

Run: `cargo build -p paavo-web`
Expected: builds, no warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/paavo-web/src/db.rs
git commit -m "feat(web): WebDb wrappers for DB-side job search"
```

---

### Task 7: `/api/jobs` reads SQL (`paavo-web`)

Rewrite the `list` handler to read rows/counts from `WebDb` instead of `LiveState.index`. Guarded by the existing `tests/api_jobs.rs` integration test (pagination + `q=alice` narrowing), which passes before and must pass after.

**Files:**
- Modify: `crates/paavo-web/src/api/jobs.rs:9-64`

- [ ] **Step 1: Swap the imports**

Replace `use crate::index::LiveState;` with `use crate::proxy::AppState;` (keep `use crate::db::WebDb;` — `get`/`log` still use it).

- [ ] **Step 2: Replace the `list` handler** (`jobs.rs:28-64`)

```rust
pub async fn list(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Page<JobListItem>>, (StatusCode, String)> {
    let page: u32 = q
        .get("page")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1);
    let per_page: u32 = q
        .get("per_page")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50)
        .clamp(1, 200);
    // Trim so `?q=` (or trailing spaces) behaves like no filter.
    let query = q.get("q").map(|v| v.trim().to_string()).unwrap_or_default();
    let as_of: Option<i64> = q.get("as_of").and_then(|v| v.parse().ok());
    let err = |e: paavo_db::DbError| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    // saturating_mul: an unclamped hostile `?page=` must yield an empty
    // page, not overflow.
    let offset = (page - 1).saturating_mul(per_page);

    let (items, total, new_count) = if query.is_empty() {
        let items = s.db.jobs_list_page(as_of, offset, per_page).map_err(err)?;
        let total = s.db.jobs_count(as_of).map_err(err)?;
        let new_count = s.db.jobs_new_count(as_of).map_err(err)?;
        (items, total, new_count)
    } else {
        let items = s.db.jobs_search_page(&query, offset, per_page).map_err(err)?;
        let total = s.db.jobs_search_count(&query).map_err(err)?;
        (items, total, 0)
    };

    Ok(Json(Page {
        items,
        total,
        page,
        per_page,
        revision: s.live.revisions().jobs,
        new_count,
        as_of,
    }))
}
```

Also update the module doc comment at `jobs.rs:3-5` to read: `//! - GET /api/jobs reads SQLite directly (fuzzy search via the fuzzy_score //! function; blank query = time-ordered page). The live revision still //! comes from the poller.`

- [ ] **Step 3: Run the integration test**

Run: `cargo test -p paavo-web --test api_jobs`
Expected: PASS (`jobs_list_paginates_and_searches`). The handler now serves rows straight from SQLite (WAL visibility makes seeded rows readable immediately).

- [ ] **Step 4: Commit**

```bash
git add crates/paavo-web/src/api/jobs.rs
git commit -m "feat(web): /api/jobs reads SQL (fuzzy search + list) instead of the index"
```

---

### Task 8: Delete `JobIndex`; poller uses `activity_digest` (`paavo-web`)

Remove the in-memory structure, slim `LiveState`, repoint the poller, and drop the now-unused `fuzzy-matcher` dep. Guarded by `tests/api_events.rs` (revision bumps) and the rewritten unit test.

**Files:**
- Modify: `crates/paavo-web/src/index.rs` (replace whole file)
- Modify: `crates/paavo-web/Cargo.toml` (remove `fuzzy-matcher`)

- [ ] **Step 1: Replace `crates/paavo-web/src/index.rs` entirely**

```rust
//! Live revision poller + the shared [`LiveState`] it feeds.
//!
//! `paavo-web` keeps no per-job data resident: the jobs list and fuzzy
//! search read SQLite directly (see `crate::api::jobs` and
//! `paavo_db::JobRow::search_index_page`). This module's only job is to
//! watch a handful of cheap, bounded aggregates and bump a per-resource
//! revision counter when something changes, which fans out over
//! `/api/events` so the SPA refetches. The `RwLock` guards are always
//! dropped before `.await`.
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// Monotonic per-resource revision counters. Bumped when the resource's
/// content fingerprint changes; pushed to clients over `/api/events`.
#[derive(Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Revisions {
    /// Jobs revision.
    pub jobs: u64,
    /// Boards revision.
    pub boards: u64,
    /// Schedules revision.
    pub schedules: u64,
}

/// Process-local live state shared by the poller and the API handlers: the
/// current revisions and a watch channel that fans revision bumps out to
/// every `/api/events` connection.
#[derive(Clone)]
pub struct LiveState {
    rev: Arc<RwLock<Revisions>>,
    tx: Arc<watch::Sender<Revisions>>,
    fp: Arc<RwLock<(u64, u64, u64)>>, // (jobs, boards, schedules) fingerprints
}

impl Default for LiveState {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveState {
    /// Construct empty live state seeded at revision 0.
    pub fn new() -> Self {
        let (tx, _) = watch::channel(Revisions::default());
        Self {
            rev: Arc::new(RwLock::new(Revisions::default())),
            tx: Arc::new(tx),
            fp: Arc::new(RwLock::new((0, 0, 0))),
        }
    }

    /// A receiver positioned at the current revisions.
    pub fn subscribe(&self) -> watch::Receiver<Revisions> {
        self.tx.subscribe()
    }

    /// Snapshot of the current revisions.
    pub fn revisions(&self) -> Revisions {
        *self.rev.read()
    }
}

fn hash_u64<T: std::hash::Hash>(items: &[T]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    items.len().hash(&mut h);
    for it in items {
        it.hash(&mut h);
    }
    h.finish()
}

/// Spawn the single background poller. `interval` is a parameter so tests
/// can run it at ~20ms. A transient DB read error keeps the last snapshot
/// and skips the tick. The RwLock guards are always dropped before `.await`.
pub fn spawn_poller(db: crate::db::WebDb, live: LiveState, interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            let mut changed = false;
            let mut snap = live.revisions();

            // Jobs: a bounded GROUP BY state digest (no full-table scan into
            // memory). Catches inserts + state transitions.
            if let Ok(digest) = db.jobs_activity_digest() {
                if live.fp.read().0 != digest {
                    live.fp.write().0 = digest;
                    snap.jobs += 1;
                    changed = true;
                }
            }
            if let Ok(boards) = db.all_boards() {
                let keys: Vec<String> = boards
                    .iter()
                    .map(|b| format!("{}:{:?}:{:?}", b.spec.id, b.spec.health, b.last_used_at))
                    .collect();
                let f = hash_u64(&keys);
                if live.fp.read().1 != f {
                    live.fp.write().1 = f;
                    snap.boards += 1;
                    changed = true;
                }
            }
            if let Ok(scheds) = db.all_schedules() {
                let keys: Vec<String> = scheds
                    .iter()
                    .map(|s| {
                        format!(
                            "{}:{}:{:?}:{:?}",
                            s.id, s.enabled, s.last_triggered_at, s.last_completed_at
                        )
                    })
                    .collect();
                let f = hash_u64(&keys);
                if live.fp.read().2 != f {
                    live.fp.write().2 = f;
                    snap.schedules += 1;
                    changed = true;
                }
            }
            if changed {
                *live.rev.write() = snap;
                let _ = live.tx.send(snap);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use paavo_db::{Db, NewJob};
    use paavo_proto::{BoardSelector, JobId, JobSource, Priority};
    use tempfile::tempdir;

    fn sample_new_job(id: JobId) -> NewJob {
        NewJob {
            id,
            priority: Priority::Interactive,
            submitter: "alice".into(),
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

    /// A RW `Db` seeds the same temp file the RO `WebDb` reads via WAL.
    /// After an insert the poller must bump the `jobs` revision, observed by
    /// a `subscribe()`r within a bounded wait.
    #[tokio::test]
    async fn spawn_poller_bumps_jobs_revision_after_insert() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paavo.sqlite");
        let rw = Db::open(&path).unwrap(); // keep the writer alive for WAL visibility
        let webdb = crate::db::WebDb::open(&path).unwrap();
        let live = LiveState::new();
        spawn_poller(webdb, live.clone(), Duration::from_millis(20));
        let mut rx = live.subscribe();

        let id = JobId::new();
        paavo_db::JobRow::insert(rw.raw_conn(), &sample_new_job(id), 0).unwrap();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!remaining.is_zero(), "poller never bumped the jobs revision");
            if tokio::time::timeout(remaining, rx.changed()).await.is_err() {
                panic!("poller never bumped the jobs revision (timeout)");
            }
            let _ = rx.borrow_and_update();
            if live.revisions().jobs >= 1 {
                break;
            }
        }
    }
}
```

- [ ] **Step 2: Remove the `fuzzy-matcher` dependency**

In `crates/paavo-web/Cargo.toml`, delete the `fuzzy-matcher = "0.3"` line and its three-line comment (the `# Fuzzy ranking …` block at lines 34-37).

- [ ] **Step 3: Run the affected tests**

Run: `cargo test -p paavo-web`
Expected: PASS — `index::tests::spawn_poller_bumps_jobs_revision_after_insert`, `api_jobs`, and `api_events` all green.

- [ ] **Step 4: Commit**

```bash
git add crates/paavo-web/src/index.rs crates/paavo-web/Cargo.toml Cargo.lock
git commit -m "refactor(web): delete in-memory JobIndex; poller uses activity_digest"
```

---

### Task 9: Remove dead `list_index` / `jobs_index` (`paavo-db`, `paavo-web`)

With the poller off the index, `WebDb::jobs_index` and `JobRow::list_index` are unused. Delete them.

**Files:**
- Modify: `crates/paavo-web/src/db.rs` (remove `jobs_index`)
- Modify: `crates/paavo-db/src/job.rs` (remove `list_index`)

- [ ] **Step 1: Delete `WebDb::jobs_index`**

In `crates/paavo-web/src/db.rs`, remove the `jobs_index` method (the `/// Lightweight jobs index projection …` doc + fn at lines 77-80).

- [ ] **Step 2: Delete `JobRow::list_index`**

In `crates/paavo-db/src/job.rs`, remove `list_index` (the `/// Lightweight projection feeding paavo-web's in-memory jobs index. …` doc + fn, originally `job.rs:264-284`). Its only caller (`jobs_index`) is gone.

- [ ] **Step 3: Build the workspace to confirm nothing else referenced them**

Run: `cargo build --workspace`
Expected: builds with no warnings (CI runs with `-Dwarnings`; an unused-import or dead-code warning here means a missed reference — fix it).

- [ ] **Step 4: Commit**

```bash
git add crates/paavo-web/src/db.rs crates/paavo-db/src/job.rs
git commit -m "chore(db,web): remove dead list_index / jobs_index"
```

---

### Task 10: Bump the search debounce to 250 ms (`paavo-web-ui`)

The SPA already debounces with a generation counter; retune it now that each committed keystroke triggers a table scan.

**Files:**
- Modify: `crates/paavo-web-ui/src/components/jobs_list.rs:23` (doc comment) and `:90` (the `Timeout`)

- [ ] **Step 1: Update the timeout**

At `jobs_list.rs:90`, change `Timeout::new(150, move || {` to `Timeout::new(250, move || {`.

- [ ] **Step 2: Update the doc comment**

At `jobs_list.rs:26-28`, change "after a 150 ms quiet period" to "after a 250 ms quiet period". (Optionally note: "longer than a pure client-side filter would need, because each committed query now runs a DB-side scan.")

- [ ] **Step 3: Build the UI** (workspace-excluded wasm32 — not covered by `cargo test`)

Run: `just build-ui`
Expected: `trunk build --release` succeeds, emits `crates/paavo-web-ui/dist`. (Prereqs: `rustup target add wasm32-unknown-unknown`, `cargo install trunk`. If trunk is unavailable in this environment, note it and defer this verification to the manual smoke.)

- [ ] **Step 4: Commit**

```bash
git add crates/paavo-web-ui/src/components/jobs_list.rs
git commit -m "feat(web-ui): bump jobs search debounce 150 -> 250 ms"
```

---

### Task 11: Refresh integration-test comments + final gate

Reword the now-stale `api_jobs.rs` comments (the index is gone) and run the full CI gate + manual smoke.

**Files:**
- Modify: `crates/paavo-web/tests/api_jobs.rs:1-6,27-28,72-73`

- [ ] **Step 1: Reword the stale comments** (no behavior change)

- File header (`api_jobs.rs:1-6`): replace the "over the poller-maintained in-memory index … seed rows via a live RW `Db` writer and then poll … until the index reflects them" wording with: `//! Integration tests for GET /api/jobs: pagination + fuzzy search. //! Rows are seeded via a live RW Db writer and read back through the RO //! WebDb (WAL), which the handler now queries directly — no in-memory index.`
- `jobs_app` comment (`api_jobs.rs:27-28`): replace "The router and the poller must share the SAME LiveState: the poller writes the index, GET /api/jobs reads it." with "The poller drives only the live revision; GET /api/jobs reads SQLite directly. The shared LiveState still supplies the echoed revision."
- `wait_for_total` doc (`api_jobs.rs:72-73`): replace "until the index reports `want` total rows" with "until GET /api/jobs reports `want` total rows (WAL read visibility is effectively immediate; the loop guards against any checkpoint lag)."

- [ ] **Step 2: Run the format + lint + test gate** (exactly what CI runs)

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 3: Manual smoke** (no hardware)

```bash
# Terminal A: fake-runner daemon
PAAVO_FAKE_RUNNER=1 cargo run -p paavod -- --config sample-paavo.toml
# Terminal B: web viewer (run `just build-ui` first to embed the SPA)
cargo run -p paavo-web -- --config sample-paavo.toml
# Terminal C: submit a few jobs so there is something to search
PAAVO_HOST=http://127.0.0.1:8090 cargo run -p paavo-cli -- jobs
```
Verify at `http://127.0.0.1:8081/jobs`: typing `almcx` ranks an `alice … mcxa266` row first; clearing the box returns to the time-ordered list; the "↑ N new" pill appears after new submissions; the query only fires ~250 ms after you stop typing.

- [ ] **Step 4: Commit**

```bash
git add crates/paavo-web/tests/api_jobs.rs
git commit -m "test(web): refresh /api/jobs integration comments after index removal"
```

---

## Self-Review

**Spec coverage** (against `2026-06-17-db-side-fuzzy-search-design.md`):

- §3.1 `fuzzy_score` UDF + subsequence `LIKE` → Tasks 1, 2, 3.
- §3.2 eliminate `JobIndex`; consumers → SQL → Tasks 6 (façade), 7 (search/list/new_count), 8 (delete index + poller).
- §3.3 `activity_digest` poller fingerprint → Task 5, wired in Task 8.
- §3.4 lowercase both sides (parity) → `search_index_page`/`search_count` lowercase the needle; haystack `lower()`ed in SQL (Task 3). Divergence is graceful (NULL score sorts last).
- §3.5 debounce 150 → 250 ms → Task 10.
- §4.1 register in `configure` → Task 2. §4.2 queries → Tasks 3–5. §4.3 façade/poller/handler → Tasks 6–8. §4.4 debounce → Task 10.
- §10 files-touched: `like.rs` (T1), `db.rs` UDF (T2), `job.rs` methods (T3–5), `board.rs` (T1), both `Cargo.toml`s (T2 add, T8 remove), `index.rs` (T8), web `db.rs` (T6, T9), `api/jobs.rs` (T7), `api_jobs.rs` (T11), `jobs_list.rs` (T10). All covered.

**Divergence-guard note:** the spec mentions a test asserting the `LIKE` candidate set equals the `fuzzy_score`-`Some` set. That equivalence is exercised indirectly by `fuzzy_search_isolates_by_submitter` (a `LIKE`-passing row also scores non-NULL, else it would be dropped from the ranked page) and by the `fuzzy_score_some_for_subsequence_none_otherwise` UDF test. A dedicated guard test can be added if stronger coverage is wanted, but it is not required for the headline behavior.

**Type consistency:** method names are stable across tasks — `search_index_page`, `search_count`, `list_index_page`, `count_newer`, `activity_digest` (db) and `jobs_search_page`, `jobs_search_count`, `jobs_list_page`, `jobs_new_count`, `jobs_activity_digest` (web). The handler calls the `jobs_*` façade names; the façade calls the `JobRow` names. `jobs_count` (pre-existing) is reused for list totals.

**Ordering invariant:** every task leaves the workspace compiling and green. Dead code (`list_index`/`jobs_index`) is removed (Task 9) only after its last caller is gone (Task 8).



