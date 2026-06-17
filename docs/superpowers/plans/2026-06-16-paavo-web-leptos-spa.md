# paavo-web v2 (Leptos CSR SPA) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace paavo-web's server-rendered HTML with a client-rendered Leptos WASM single-page app (live updates everywhere, server-side fuzzy search over full history, pagination) while keeping paavo-web a single binary.

**Architecture:** paavo-web stays one axum binary exposing a JSON + SSE API over the read-only SQLite, plus an embedded Leptos CSR app (built by `trunk`, embedded via `rust-embed`). A background poller maintains an in-memory job index + per-resource revisions; a consolidated `/api/events` SSE pushes revision bumps; the client refetches its current view. The per-job log proxy to paavod is unchanged.

**Tech Stack:** Rust 1.95, axum 0.7, Leptos (CSR), `trunk`, `gloo-net`, `rust-embed`, `nucleo-matcher`/`fuzzy-matcher`, rusqlite (RO/WAL), hand-rolled semantic CSS.

**Spec:** `docs/superpowers/specs/2026-06-16-paavo-web-leptos-spa-design.md` (read it first).

---

## Conventions for the executor

- **Worktree:** all work happens in the worktree `.worktrees/paavo-web-leptos-spa` on branch `feat/paavo-web-leptos-spa`. Run every command from that directory.
- **TDD:** for library/server code, write the failing test first, watch it fail, implement minimally, watch it pass, commit. The UI crate (`paavo-web-ui`) is exercised by a handful of `wasm-bindgen-test` unit tests + a manual smoke checklist (no headless-browser harness — house style).
- **Gates (run before every commit that touches workspace crates):**
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
  CI sets `RUSTFLAGS="-Dwarnings"`. The UI crate is **excluded** from the workspace; lint/format it in its own dir (`cargo fmt`, `cargo clippy` run inside `crates/paavo-web-ui`).
- **Commits:** Conventional Commits with a crate scope, e.g. `feat(proto): add JobListItem/ScheduleView/Page wire types`. Small, scoped commits per task.
- **System deps:** `libudev-dev` + `pkg-config` (probe-rs). For the UI: `rustup target add wasm32-unknown-unknown` and `cargo install trunk`.

## File structure map

| File | Responsibility |
|---|---|
| `crates/paavo-proto/src/job.rs` | + `JobListItem` (lightweight list row) |
| `crates/paavo-proto/src/schedule.rs` (**new**) | `ScheduleView` wire type |
| `crates/paavo-proto/src/page.rs` (**new**) | generic `Page<T>` envelope |
| `crates/paavo-proto/src/lib.rs` | export the new types; `mod schedule; mod page;` |
| `crates/paavo-db/src/job.rs` | + `list_index`, `list_page`, `count` (as_of-aware) |
| `crates/paavo-db/src/board.rs` | + `list_page`, `count` |
| `crates/paavo-db/src/schedule.rs` | + `list_page`, `count` |
| `crates/paavo-web/src/db.rs` | thin RO wrappers for the new queries |
| `crates/paavo-web/src/index.rs` (**new**) | `JobIndex`, `Revisions`, poller, search/paginate |
| `crates/paavo-web/src/api/mod.rs` (**new**) | `jobs.rs`, `boards.rs`, `schedules.rs`, `events.rs` |
| `crates/paavo-web/src/embed.rs` (**new**) | rust-embed asset serving + SPA fallback + not-built placeholder |
| `crates/paavo-web/src/app.rs` | new route table; drop page/feed routes |
| `crates/paavo-web/src/{pages,feed}.rs`, `assets/*` | **removed** |
| `crates/paavo-web/src/proxy.rs`, `config.rs`, `time.rs` | kept (time.rs only if a handler still needs it) |
| `crates/paavo-web-ui/**` (**new, excluded crate**) | Leptos CSR app, `Trunk.toml`, `index.html`, CSS, components, `api`/`live` |
| `dev/seed-demo/**` (**new, excluded crate**) | DB seeder for UI stress-testing |
| `Cargo.toml` (root) | add `crates/paavo-web-ui`, `dev/seed-demo` to `exclude` |
| `justfile` (**new**) | `build-ui`, `seed-demo`, `dev` recipes |
| `.github/workflows/*` | add wasm target + trunk + `just build-ui` step |
| `AGENTS.md` | crate map + build step + drift-note updates |

---

## Phase 0 — Baseline & spikes

### Task 0.1: Confirm green baseline

- [ ] **Step 1: Run the full suite** — `cargo test --workspace`. Expected: PASS (deterministic, no hardware). If anything fails, stop and report before changing code.

### Task 0.2: Spike — UI crate skeleton, Leptos pin, trunk, ulid-on-wasm, embed

Goal: produce a minimal embedded SPA that prints a parsed `paavo_proto::JobId`, proving the whole toolchain. **Record the pinned versions in `crates/paavo-web-ui/Cargo.toml`** — later UI tasks target them.

- [ ] **Step 1: Scaffold the crate** — create `crates/paavo-web-ui/` with:
  - `Cargo.toml`:
    ```toml
    [package]
    name = "paavo-web-ui"
    version = "0.1.0"
    edition = "2021"
    publish = false

    [dependencies]
    leptos = { version = "0.7", features = ["csr"] }
    leptos_router = "0.7"
    paavo-proto = { path = "../paavo-proto" }
    gloo-net = "0.6"
    gloo-timers = { version = "0.3", features = ["futures"] }
    wasm-bindgen = "0.2"
    wasm-bindgen-futures = "0.4"
    web-sys = { version = "0.3", features = ["EventSource", "MessageEvent", "Storage", "Window", "HtmlInputElement"] }
    serde = { version = "1", features = ["derive"] }
    serde_json = "1"
    console_error_panic_hook = "0.1"
    # ulid (via paavo-proto) needs a wasm RNG backend to link, even though
    # the UI only PARSES ulids. Enable getrandom's js backend by feature
    # unification:
    getrandom = { version = "0.2", features = ["js"] }

    [dev-dependencies]
    wasm-bindgen-test = "0.3"
    ```
  - `index.html`:
    ```html
    <!doctype html>
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>paavo</title>
        <link data-trunk rel="rust" data-wasm-opt="z" />
        <link data-trunk rel="css" href="style.css" />
      </head>
      <body></body>
    </html>
    ```
  - `Trunk.toml`:
    ```toml
    [build]
    dist = "dist"
    ```
  - `src/main.rs`:
    ```rust
    use leptos::prelude::*;
    fn main() {
        console_error_panic_hook::set_once();
        leptos::mount::mount_to_body(|| {
            let id = "01JZ8K3Q9FXM2H7B4N0PXR5T6A".parse::<paavo_proto::JobId>();
            let text = match id { Ok(j) => j.to_string(), Err(_) => "parse-failed".into() };
            view! { <p>"hello paavo: " {text}</p> }
        });
    }
    ```
  - `style.css`: `body { font-family: system-ui; }`
- [ ] **Step 2: Add to workspace excludes** — in root `Cargo.toml` `exclude = [...]` add `"crates/paavo-web-ui"` with a comment: `# Leptos CSR app; cross-compiles to wasm32, built by trunk (see AGENTS.md).`
- [ ] **Step 3: Build it** — from `crates/paavo-web-ui`: `trunk build`. Expected: `dist/` contains `index.html`, a `*_bg.wasm`, a JS shim, `style-*.css`. If `ulid`/`getrandom` fails to link, confirm the `getrandom` `js` feature is active; if Leptos 0.7 fails on Rust 1.95, pin the latest 0.7.x that builds and **record it**.
- [ ] **Step 4: Confirm `cargo build --workspace` still works** (UI crate excluded, so the workspace is unaffected). Expected: PASS.
- [ ] **Step 5: Commit** — `git add crates/paavo-web-ui Cargo.toml && git commit -m "feat(web-ui): spike Leptos CSR skeleton + trunk + ulid-on-wasm"`

### Task 0.3: Spike — pick the fuzzy matcher

- [ ] **Step 1:** In a scratch `cargo add`/throwaway test, compare `nucleo-matcher` and `fuzzy-matcher` (`SkimMatcherV2`) for: ranking `"almcx"` against `["alice mcxa266-01", "bob mcxa266-02", "cron mcxa266-03"]`. Pick the one with better ranking + simpler API. **Record the choice**; Phase 3 uses it. Default if undecided: `fuzzy-matcher = "0.3"` (`SkimMatcherV2`, returns `Option<i64>` score).
- [ ] **Step 2:** No commit (spike only); the dependency is added in Task 3.2.

---

## Phase 1 — paavo-proto view types

### Task 1.1: `JobListItem`

**Files:** Modify `crates/paavo-proto/src/job.rs`; Test: same file `#[cfg(test)]`.

- [ ] **Step 1: Failing test** — add to `job.rs` tests:
  ```rust
  #[test]
  fn job_list_item_roundtrips() {
      let it = JobListItem {
          id: JobId::new(),
          state: JobState::Running,
          priority: Priority::Interactive,
          submitter: "alice".into(),
          board_id: Some("mcxa266-01".into()),
          submitted_at: 1_700_000_000_000,
      };
      let j = serde_json::to_string(&it).unwrap();
      assert_eq!(it, serde_json::from_str::<JobListItem>(&j).unwrap());
  }
  ```
- [ ] **Step 2: Run, expect fail** — `cargo test -p paavo-proto job_list_item_roundtrips` → FAIL (`JobListItem` undefined).
- [ ] **Step 3: Implement** — add to `job.rs`:
  ```rust
  /// Lightweight jobs-list row. A subset of [`JobView`]: exactly the
  /// columns the jobs index holds in memory and the list renders. The
  /// fuzzy haystack is built from `id + submitter + state + board_id`.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  pub struct JobListItem {
      /// Job id.
      pub id: JobId,
      /// Current state.
      pub state: JobState,
      /// Scheduler priority.
      pub priority: Priority,
      /// Submitter free text.
      pub submitter: String,
      /// Board the job is/was dispatched to, if any.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub board_id: Option<String>,
      /// Submission time, epoch ms.
      pub submitted_at: i64,
  }
  ```
- [ ] **Step 4:** Export in `lib.rs`: add `JobListItem` to the `pub use job::{...}` list.
- [ ] **Step 5: Run, expect pass** — `cargo test -p paavo-proto job_list_item_roundtrips` → PASS.
- [ ] **Step 6: Commit** — `feat(proto): add JobListItem list-row wire type`.

### Task 1.2: `ScheduleView`

**Files:** Create `crates/paavo-proto/src/schedule.rs`; Modify `lib.rs`.

- [ ] **Step 1: Failing test** — create `schedule.rs` with the type + a roundtrip test:
  ```rust
  //! Schedule wire view. Mirrors the public fields of paavo-db's
  //! `ScheduleRow` (no server-local fields to drop).
  use serde::{Deserialize, Serialize};

  /// JSON shape for a cron schedule row, served by paavo-web's
  /// `GET /api/schedules`.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  pub struct ScheduleView {
      /// Schedule id, e.g. `nightly`.
      pub id: String,
      /// Cron expression.
      pub cron: String,
      /// Whether the schedule is active.
      pub enabled: bool,
      /// Last firing time, epoch ms.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub last_triggered_at: Option<i64>,
      /// Last completion time, epoch ms.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub last_completed_at: Option<i64>,
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn schedule_view_roundtrips() {
          let s = ScheduleView { id: "nightly".into(), cron: "0 0 19 * * *".into(),
              enabled: true, last_triggered_at: Some(1), last_completed_at: None };
          let j = serde_json::to_string(&s).unwrap();
          assert_eq!(s, serde_json::from_str::<ScheduleView>(&j).unwrap());
      }
  }
  ```
- [ ] **Step 2:** In `lib.rs` add `mod schedule;` and `pub use schedule::ScheduleView;`.
- [ ] **Step 3: Run** — `cargo test -p paavo-proto schedule_view` → PASS.
- [ ] **Step 4: Commit** — `feat(proto): add ScheduleView wire type`.

### Task 1.3: `Page<T>`

**Files:** Create `crates/paavo-proto/src/page.rs`; Modify `lib.rs`.

- [ ] **Step 1: Implement + test** — create `page.rs`:
  ```rust
  //! Generic pagination envelope for list endpoints.
  use serde::{Deserialize, Serialize};

  /// One page of a list endpoint plus the metadata the client needs for
  /// pagination + live updates.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  pub struct Page<T> {
      /// The rows on this page.
      pub items: Vec<T>,
      /// Total rows across all pages (after filtering, before paging).
      pub total: u64,
      /// 1-based page number echoed back.
      pub page: u32,
      /// Page size echoed back.
      pub per_page: u32,
      /// Resource revision at query time (for live de-dup).
      pub revision: u64,
      /// Jobs newer than `as_of` (0 for boards/schedules/search mode).
      pub new_count: u64,
      /// Echoed jobs cursor (epoch-ms); None for boards/schedules.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub as_of: Option<i64>,
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      #[test]
      fn page_roundtrips() {
          let p = Page { items: vec![1u32, 2, 3], total: 3, page: 1, per_page: 50,
              revision: 7, new_count: 0, as_of: Some(123) };
          let j = serde_json::to_string(&p).unwrap();
          assert_eq!(p, serde_json::from_str::<Page<u32>>(&j).unwrap());
      }
  }
  ```
- [ ] **Step 2:** `lib.rs`: `mod page;` + `pub use page::Page;`.
- [ ] **Step 3: Run** — `cargo test -p paavo-proto page_roundtrips` → PASS.
- [ ] **Step 4: Commit** — `feat(proto): add generic Page<T> envelope`.

---

## Phase 2 — paavo-db pagination & index queries

> All four tasks follow the existing `from_row`/`query_map` patterns in each file. Use `tempfile` + a RW `Db::open` to seed, then assert (the RO read path is the same connection type).

### Task 2.1: `JobRow::list_index`

**Files:** Modify `crates/paavo-db/src/job.rs`.

- [ ] **Step 1: Failing test** — in `job.rs` tests, insert 2 jobs (reuse the existing `sample_new_job`-style helper there) and assert `list_index` returns 2 `JobListItem`s newest-first with the right ids/states.
- [ ] **Step 2: Implement:**
  ```rust
  /// Lightweight projection feeding paavo-web's in-memory jobs index.
  /// Only the columns the list/search need; newest-first.
  pub fn list_index(conn: &Connection) -> Result<Vec<paavo_proto::JobListItem>> {
      let mut stmt = conn.prepare(
          "SELECT id, state, priority, submitter, board_id, submitted_at
           FROM job ORDER BY submitted_at DESC, id DESC",
      )?;
      let rows = stmt
          .query_map([], |r| Ok(index_row(r)))?
          .collect::<std::result::Result<Vec<_>, _>>()?
          .into_iter()
          .collect::<Result<Vec<_>>>()?;
      Ok(rows)
  }
  ```
  and a helper:
  ```rust
  fn index_row(r: &Row<'_>) -> Result<paavo_proto::JobListItem> {
      let id = JobId::from_str(&r.get::<_, String>("id")?).map_err(|_| DbError::UnknownEnum {
          column: "job.id", value: "bad ulid".into() })?;
      Ok(paavo_proto::JobListItem {
          id,
          state: state_from_str(&r.get::<_, String>("state")?)?,
          priority: priority_from_i64(r.get::<_, i64>("priority")?)?,
          submitter: r.get("submitter")?,
          board_id: r.get("board_id")?,
          submitted_at: r.get("submitted_at")?,
      })
  }
  ```
  (Note: `query_map`'s closure returns `rusqlite::Result<Result<JobListItem>>`; mirror the existing `from_row` double-Result handling — wrap `index_row` like the existing code does.)
- [ ] **Step 3: Run** — `cargo test -p paavo-db list_index` → PASS. **Step 4: Commit** — `feat(db): JobRow::list_index projection for the web jobs index`.

### Task 2.2: `JobRow::list_page` + `count` (as_of-aware)

**Files:** Modify `crates/paavo-db/src/job.rs`.

- [ ] **Step 1: Failing test** — insert 3 jobs at `submitted_at` 100/200/300; assert:
  - `count(None) == 3`; `count(Some(250)) == 2`.
  - `list_page(None, 0, 2)` returns the two newest (300, 200).
  - `list_page(Some(150), 0, 10)` returns only the `submitted_at <= 150` job (100).
- [ ] **Step 2: Implement:**
  ```rust
  /// Page of full job rows, newest-first, optionally pinned to
  /// `submitted_at <= as_of` for stable pagination under live inserts.
  pub fn list_page(conn: &Connection, as_of: Option<i64>, offset: u32, limit: u32)
      -> Result<Vec<Self>> {
      let (sql, bind): (&str, Vec<i64>) = match as_of {
          Some(t) => ("SELECT * FROM job WHERE submitted_at <= ?1
                       ORDER BY submitted_at DESC, id DESC LIMIT ?2 OFFSET ?3",
                      vec![t, limit as i64, offset as i64]),
          None    => ("SELECT * FROM job
                       ORDER BY submitted_at DESC, id DESC LIMIT ?1 OFFSET ?2",
                      vec![limit as i64, offset as i64]),
      };
      let mut stmt = conn.prepare(sql)?;
      let rows = stmt.query_map(rusqlite::params_from_iter(bind), from_row)?
          .collect::<std::result::Result<Vec<_>, _>>()?
          .into_iter().collect::<Result<Vec<_>>>()?;
      Ok(rows)
  }

  /// Count of jobs (optionally `submitted_at <= as_of`).
  pub fn count(conn: &Connection, as_of: Option<i64>) -> Result<u64> {
      let n: i64 = match as_of {
          Some(t) => conn.query_row("SELECT COUNT(*) FROM job WHERE submitted_at <= ?1",
                                    params![t], |r| r.get(0))?,
          None => conn.query_row("SELECT COUNT(*) FROM job", [], |r| r.get(0))?,
      };
      Ok(n as u64)
  }
  ```
- [ ] **Step 3: Run** — PASS. **Step 4: Commit** — `feat(db): JobRow::list_page/count with as_of pin`.

### Task 2.3: `BoardRow::list_page` + `count`

**Files:** Modify `crates/paavo-db/src/board.rs`.

- [ ] **Step 1: Failing test** — insert 3 boards; `count() == 3`; `list_page(0,2)` returns 2 ordered by id ASC.
- [ ] **Step 2: Implement** (mirror `list_all`, ordered `id ASC`):
  ```rust
  pub fn list_page(conn: &Connection, offset: u32, limit: u32) -> Result<Vec<Self>> {
      let mut stmt = conn.prepare("SELECT * FROM board ORDER BY id ASC LIMIT ?1 OFFSET ?2")?;
      let rows = stmt.query_map(params![limit as i64, offset as i64], from_row)?
          .collect::<std::result::Result<Vec<_>, _>>()?
          .into_iter().collect::<Result<Vec<_>>>()?;
      Ok(rows)
  }
  pub fn count(conn: &Connection) -> Result<u64> {
      let n: i64 = conn.query_row("SELECT COUNT(*) FROM board", [], |r| r.get(0))?;
      Ok(n as u64)
  }
  ```
- [ ] **Step 3: Run** — PASS. **Step 4: Commit** — `feat(db): BoardRow::list_page/count`.

### Task 2.4: `ScheduleRow::list_page` + `count`

**Files:** Modify `crates/paavo-db/src/schedule.rs`.

- [ ] **Step 1: Failing test** — upsert 2 schedules; `count() == 2`; `list_page(0,1)` returns 1 (id ASC).
- [ ] **Step 2: Implement** (reuse `row_to_schedule`):
  ```rust
  pub fn list_page(conn: &Connection, offset: u32, limit: u32) -> Result<Vec<ScheduleRow>> {
      let mut stmt = conn.prepare(
          "SELECT id, cron, enabled, last_triggered_at, last_completed_at
           FROM schedule ORDER BY id ASC LIMIT ?1 OFFSET ?2")?;
      let rows = stmt.query_map(params![limit as i64, offset as i64], row_to_schedule)?
          .collect::<std::result::Result<Vec<_>, _>>()?;
      Ok(rows)
  }
  pub fn count(conn: &Connection) -> Result<u64> {
      let n: i64 = conn.query_row("SELECT COUNT(*) FROM schedule", [], |r| r.get(0))?;
      Ok(n as u64)
  }
  ```
- [ ] **Step 3: Run** — PASS. **Step 4: Commit** — `feat(db): ScheduleRow::list_page/count`.

---

## Phase 3 — paavo-web server (API + index + events + embed)

> Add deps to `crates/paavo-web/Cargo.toml`: `rust-embed = "8"`, the chosen fuzzy matcher (e.g. `fuzzy-matcher = "0.3"`). Remove now-unused deps at the end (Task 3.9).

### Task 3.1: `WebDb` wrappers

**Files:** Modify `crates/paavo-web/src/db.rs`.

- [ ] **Step 1:** Add wrappers (sync, short-lived lock — same posture as today):
  ```rust
  pub fn jobs_index(&self) -> paavo_db::Result<Vec<paavo_proto::JobListItem>> {
      paavo_db::JobRow::list_index(self.inner.lock().raw_conn())
  }
  pub fn jobs_page(&self, as_of: Option<i64>, offset: u32, limit: u32)
      -> paavo_db::Result<Vec<paavo_db::JobRow>> {
      paavo_db::JobRow::list_page(self.inner.lock().raw_conn(), as_of, offset, limit)
  }
  pub fn jobs_count(&self, as_of: Option<i64>) -> paavo_db::Result<u64> {
      paavo_db::JobRow::count(self.inner.lock().raw_conn(), as_of)
  }
  pub fn boards_page(&self, offset: u32, limit: u32) -> paavo_db::Result<Vec<paavo_db::BoardRow>> {
      paavo_db::BoardRow::list_page(self.inner.lock().raw_conn(), offset, limit)
  }
  pub fn boards_count(&self) -> paavo_db::Result<u64> {
      paavo_db::BoardRow::count(self.inner.lock().raw_conn())
  }
  pub fn schedules_page(&self, offset: u32, limit: u32) -> paavo_db::Result<Vec<paavo_db::ScheduleRow>> {
      paavo_db::ScheduleRow::list_page(self.inner.lock().raw_conn(), offset, limit)
  }
  pub fn schedules_count(&self) -> paavo_db::Result<u64> {
      paavo_db::ScheduleRow::count(self.inner.lock().raw_conn())
  }
  ```
- [ ] **Step 2:** `cargo build -p paavo-web` → PASS. **Step 3: Commit** — `feat(web): WebDb pagination/index wrappers`.

### Task 3.2: `JobIndex` + `Revisions` + search/paginate

**Files:** Create `crates/paavo-web/src/index.rs`; add `pub mod index;` to `lib.rs`.

- [ ] **Step 1: Failing test** — in `index.rs` `#[cfg(test)]`: build a `JobIndex` from a `Vec<JobListItem>` (alice/mcxa266-01, bob/mcxa266-02, cron/mcxa266-03); assert:
  - `search("", as_of=None, 1, 50)` returns all 3 newest-first, `total==3`.
  - `search("alice", None, 1, 50)` ranks the alice row first, `total==1`.
  - paging: `search("", None, 2, 2)` returns 1 item with `total==3`.
- [ ] **Step 2: Implement** (`SkimMatcherV2` from `fuzzy_matcher`):
  ```rust
  use fuzzy_matcher::skim::SkimMatcherV2;
  use fuzzy_matcher::FuzzyMatcher;
  use paavo_proto::JobListItem;

  /// In-memory jobs index: list items + a lowercased haystack each.
  #[derive(Default, Clone)]
  pub struct JobIndex { items: Vec<JobListItem>, haystacks: Vec<String> }

  impl JobIndex {
      pub fn from_items(items: Vec<JobListItem>) -> Self {
          let haystacks = items.iter().map(haystack).collect();
          Self { items, haystacks }
      }
      /// (page_items, total). `q` blank => time-ordered (already newest-first),
      /// optionally pinned to submitted_at <= as_of. Non-blank => fuzzy-ranked.
      pub fn search(&self, q: &str, as_of: Option<i64>, page: u32, per_page: u32)
          -> (Vec<JobListItem>, u64) {
          let matched: Vec<&JobListItem> = if q.trim().is_empty() {
              self.items.iter()
                  .filter(|it| as_of.map_or(true, |t| it.submitted_at <= t))
                  .collect()
          } else {
              let m = SkimMatcherV2::default();
              let mut scored: Vec<(i64, &JobListItem)> = self.items.iter().enumerate()
                  .filter_map(|(i, it)| m.fuzzy_match(&self.haystacks[i], q).map(|s| (s, it)))
                  .collect();
              // score desc, then newest-first (items already newest-first → stable)
              scored.sort_by(|a, b| b.0.cmp(&a.0));
              scored.into_iter().map(|(_, it)| it).collect()
          };
          let total = matched.len() as u64;
          let start = ((page.saturating_sub(1)) * per_page) as usize;
          let items = matched.into_iter().skip(start).take(per_page as usize).cloned().collect();
          (items, total)
      }
      pub fn new_count(&self, as_of: Option<i64>) -> u64 {
          match as_of { Some(t) => self.items.iter().filter(|it| it.submitted_at > t).count() as u64, None => 0 }
      }
  }

  fn haystack(it: &JobListItem) -> String {
      format!("{} {} {:?} {}", it.id, it.submitter, it.state, it.board_id.as_deref().unwrap_or("")).to_lowercase()
  }
  ```
- [ ] **Step 3: Run** — `cargo test -p paavo-web index::` → PASS. **Step 4: Commit** — `feat(web): in-memory JobIndex with fuzzy search + paging`.

### Task 3.3: `Revisions` + generalized poller

**Files:** Modify `crates/paavo-web/src/index.rs`.

- [ ] **Step 1:** Add a shared state holder + poller. The poller recomputes the index + per-resource fingerprints each tick; on change it bumps a revision and publishes via `tokio::sync::watch` (latest-wins, like today's feed).
  ```rust
  use parking_lot::RwLock;
  use std::sync::Arc;
  use std::time::Duration;
  use tokio::sync::watch;

  #[derive(Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
  pub struct Revisions { pub jobs: u64, pub boards: u64, pub schedules: u64 }

  #[derive(Clone)]
  pub struct LiveState {
      pub index: Arc<RwLock<JobIndex>>,
      pub rev: Arc<RwLock<Revisions>>,
      tx: Arc<watch::Sender<Revisions>>,
      fp: Arc<RwLock<(u64, u64, u64)>>, // last fingerprints
  }
  impl LiveState {
      pub fn new() -> Self {
          let (tx, _) = watch::channel(Revisions::default());
          Self { index: Default::default(), rev: Default::default(), tx: Arc::new(tx), fp: Default::default() }
      }
      pub fn subscribe(&self) -> watch::Receiver<Revisions> { self.tx.subscribe() }
      pub fn revisions(&self) -> Revisions { *self.rev.read() }
  }

  /// Compute a cheap content fingerprint for a resource.
  fn fp_jobs(items: &[JobListItem]) -> u64 {
      use std::hash::{Hash, Hasher};
      let mut h = std::collections::hash_map::DefaultHasher::new();
      items.len().hash(&mut h);
      for it in items { it.id.to_string().hash(&mut h); format!("{:?}", it.state).hash(&mut h); }
      h.finish()
  }
  // fp_boards / fp_schedules: hash (id, health/enabled, last_used/last_completed) similarly.

  pub fn spawn_poller(db: crate::db::WebDb, live: LiveState, interval: Duration) {
      tokio::spawn(async move {
          let mut t = tokio::time::interval(interval);
          loop {
              t.tick().await;
              // jobs
              if let Ok(items) = db.jobs_index() {
                  let f = fp_jobs(&items);
                  let mut fp = live.fp.write();
                  if fp.0 != f {
                      fp.0 = f; drop(fp);
                      *live.index.write() = JobIndex::from_items(items);
                      let mut r = live.rev.write(); r.jobs += 1; let snap = *r; drop(r);
                      let _ = live.tx.send(snap);
                  }
              }
              // boards + schedules: same shape, bump r.boards / r.schedules, send snapshot.
          }
      });
  }
  ```
  (Implement `fp_boards`/`fp_schedules` + their branches; each bump sends one combined `Revisions` snapshot. Never hold an `RwLock` guard across `.await`.)
- [ ] **Step 2: Test** — `spawn_poller` at ~20ms: after inserting a job into a RW handle on the same file, `subscribe()` observes `jobs` increment within a bounded timeout (mirror `tests/feed.rs::spawn_poller_pushes_after_insert`).
- [ ] **Step 3: Run** — PASS. **Step 4: Commit** — `feat(web): generalized poller with per-resource revisions`.

### Task 3.4: `GET /api/jobs`

**Files:** Create `crates/paavo-web/src/api/mod.rs` (`pub mod jobs; ...`) and `api/jobs.rs`; `lib.rs`: `pub mod api;`.

- [ ] **Step 1: Failing integration test** — `tests/api_jobs.rs`: build the router (Task 3.9 wiring) over a temp DB seeded with 3 jobs; `GET /api/jobs?per_page=2&page=1` → 200, JSON `Page<JobListItem>` with `items.len()==2`, `total==3`, `revision>=0`. `GET /api/jobs?q=alice` → only alice. (Use the existing harness style from `tests/feed.rs`.)
- [ ] **Step 2: Implement** `api/jobs.rs`:
  ```rust
  use crate::index::LiveState;
  use axum::extract::{Query, State};
  use axum::Json;
  use paavo_proto::{JobListItem, Page};
  use std::collections::HashMap;

  pub async fn list(State(live): State<LiveState>, Query(q): Query<HashMap<String, String>>)
      -> Json<Page<JobListItem>> {
      let page: u32 = q.get("page").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
      let per_page: u32 = q.get("per_page").and_then(|v| v.parse().ok()).unwrap_or(50).clamp(1, 200);
      let query = q.get("q").cloned().unwrap_or_default();
      let as_of: Option<i64> = q.get("as_of").and_then(|v| v.parse().ok());
      let (items, total, new_count, revision) = {
          let idx = live.index.read();
          let (items, total) = idx.search(&query, as_of, page, per_page);
          let new_count = if query.trim().is_empty() { idx.new_count(as_of) } else { 0 };
          (items, total, new_count, live.revisions().jobs)
      };
      Json(Page { items, total, page, per_page, revision, new_count, as_of })
  }
  ```
  Add `impl FromRef<AppState> for LiveState` in `app.rs` (Task 3.9).
- [ ] **Step 3: Run** — PASS. **Step 4: Commit** — `feat(web): GET /api/jobs (paginated fuzzy search)`.

### Task 3.5: `GET /api/jobs/:id` and `/api/jobs/:id/log`

**Files:** `api/jobs.rs`.

- [ ] **Step 1: Failing test** — seed one job + a couple log frames; `GET /api/jobs/<id>` → `JobView`; `GET /api/jobs/<id>/log?limit=10` → `Vec<LogFrame>`; bad id → 400; unknown id → 404.
- [ ] **Step 2: Implement** — `get` reads `WebDb::job(&id)` → map to `JobView` via a `row_to_view` helper (copy paavod's `routes/jobs.rs::row_to_view`); `log` uses `WebDb::job_logs(&id, limit)` (existing) with an `offset` query param (add an offset-aware `WebDb::job_logs_page` wrapping `LogFrame::list(conn, id, offset, limit)`).
- [ ] **Step 3: Run** — PASS. **Step 4: Commit** — `feat(web): GET /api/jobs/:id and /:id/log`.

### Task 3.6: `GET /api/boards` and `/api/schedules`

**Files:** `api/boards.rs`, `api/schedules.rs`.

- [ ] **Step 1: Failing test** — seed 2 boards + 2 schedules; `GET /api/boards?per_page=20` → `Page<BoardView>` total 2; `GET /api/schedules` → `Page<ScheduleView>` total 2.
- [ ] **Step 2: Implement** — `boards.rs`: page via `WebDb::boards_page/boards_count`, map `BoardRow`→`BoardView` (copy paavod's `row_to_view` from `routes/boards.rs:143`), default `per_page=20` clamp ≤100, `revision = live.revisions().boards`, `new_count=0`, `as_of=None`. `schedules.rs`: same with `ScheduleRow`→`ScheduleView { id, cron, enabled, last_triggered_at, last_completed_at }`, `per_page=20`.
- [ ] **Step 3: Run** — PASS. **Step 4: Commit** — `feat(web): GET /api/boards and /api/schedules (paginated)`.

### Task 3.7: `GET /api/events` (consolidated SSE)

**Files:** `api/events.rs`.

- [ ] **Step 1: Failing test** — `tests/api_events.rs`: with the poller at ~20ms, connect to `/api/events`; assert an immediate snapshot event, then after inserting a job a `jobs` event arrives within a bounded timeout. Content-type `text/event-stream`.
- [ ] **Step 2: Implement** (model on `feed.rs::dashboard_feed`): subscribe to `LiveState`, emit an immediate snapshot (`event: snapshot`, data = `Revisions` JSON), then on each `changed()` diff the new vs previous `Revisions` and emit one named event per changed resource (`event: jobs|boards|schedules`, `data: {"revision":N}`). 15s keep-alive.
  ```rust
  pub async fn events(State(live): State<LiveState>) -> impl IntoResponse {
      let mut rx = live.subscribe();
      let stream = async_stream::stream! {
          let mut prev = *rx.borrow_and_update();
          yield Ok::<_, std::convert::Infallible>(
              Event::default().event("snapshot").json_data(prev).unwrap());
          while rx.changed().await.is_ok() {
              let cur = *rx.borrow_and_update();
              if cur.jobs != prev.jobs { yield Ok(Event::default().event("jobs").data(cur.jobs.to_string())); }
              if cur.boards != prev.boards { yield Ok(Event::default().event("boards").data(cur.boards.to_string())); }
              if cur.schedules != prev.schedules { yield Ok(Event::default().event("schedules").data(cur.schedules.to_string())); }
              prev = cur;
          }
      };
      Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keep-alive"))
  }
  ```
- [ ] **Step 3: Run** — PASS. **Step 4: Commit** — `feat(web): consolidated /api/events SSE (per-resource revisions)`.

### Task 3.8: Embedded assets + SPA fallback

**Files:** Create `crates/paavo-web/src/embed.rs`; `lib.rs`: `pub mod embed;`.

- [ ] **Step 1: Implement** — `rust-embed` over the UI dist with a not-built fallback:
  ```rust
  use axum::http::{header, StatusCode, Uri};
  use axum::response::{Html, IntoResponse, Response};

  #[derive(rust_embed::RustEmbed)]
  #[folder = "../paavo-web-ui/dist"]
  struct Assets;

  const NOT_BUILT: &str = "<h1>paavo-web UI not built</h1><p>Run <code>just build-ui</code>.</p>";

  /// Serve an embedded asset by path, or fall back to index.html (SPA),
  /// or a placeholder if the UI was never built.
  pub async fn serve(uri: Uri) -> Response {
      let path = uri.path().trim_start_matches('/');
      let path = if path.is_empty() { "index.html" } else { path };
      if let Some(f) = Assets::get(path) {
          let mime = mime_for(path);
          return ([(header::CONTENT_TYPE, mime)], f.data.into_owned()).into_response();
      }
      // SPA fallback: serve index.html for unknown non-asset routes.
      match Assets::get("index.html") {
          Some(f) => Html(f.data.into_owned()).into_response(),
          None => (StatusCode::OK, Html(NOT_BUILT)).into_response(),
      }
  }
  fn mime_for(p: &str) -> &'static str {
      if p.ends_with(".wasm") { "application/wasm" }
      else if p.ends_with(".js") { "application/javascript; charset=utf-8" }
      else if p.ends_with(".css") { "text/css; charset=utf-8" }
      else if p.ends_with(".html") { "text/html; charset=utf-8" }
      else { "application/octet-stream" }
  }
  ```
  (rust-embed requires `../paavo-web-ui/dist` to exist at compile time; the Task 0.2 spike created it. If empty, `Assets::get` returns None and we serve `NOT_BUILT` — handled.)
- [ ] **Step 2: Test** — `tests/embed.rs`: `GET /` returns 200 HTML; `GET /jobs` (unknown asset) returns 200 HTML (SPA fallback). **Step 3: Commit** — `feat(web): embed UI dist + SPA fallback`.

### Task 3.9: Rewire router, drop SSR pages/feed, rewrite tests

**Files:** `crates/paavo-web/src/app.rs`, `proxy.rs` (AppState), `main.rs`, `lib.rs`; **remove** `pages/`, `feed.rs`, `assets/`; rewrite `tests/smoke.rs`, `tests/feed.rs`; keep `tests/proxy.rs`.

- [ ] **Step 1:** New `AppState { db, paavod, live }` (replace `feed: JobFeed` with `live: LiveState`). Add `FromRef<AppState> for LiveState`. Remove the `JobFeed` `FromRef`.
- [ ] **Step 2:** New route table in `app.rs`:
  ```rust
  Router::new()
      .route("/api/jobs", get(crate::api::jobs::list))
      .route("/api/jobs/:id", get(crate::api::jobs::get))
      .route("/api/jobs/:id/log", get(crate::api::jobs::log))
      .route("/api/jobs/:id/stream", get(crate::proxy::stream_job)) // kept
      .route("/api/boards", get(crate::api::boards::list))
      .route("/api/schedules", get(crate::api::schedules::list))
      .route("/api/events", get(crate::api::events::events))
      .fallback(crate::embed::serve) // index.html + assets + SPA fallback
      .with_state(state)
  ```
  Remove the page routes, `/api/dashboard/feed`, and the static CSS/JS routes.
- [ ] **Step 3:** `main.rs`: replace the feed seed/poller with `let live = LiveState::new(); spawn_poller(db.clone(), live.clone(), Duration::from_secs(1)); let state = AppState { db, paavod, live };`. Drop `pages`/`feed` module decls from `lib.rs`; add `index`, `api`, `embed`.
- [ ] **Step 4:** `git rm` `src/pages/`, `src/feed.rs`, `src/assets/`. Rewrite `tests/smoke.rs` to assert `GET /` serves HTML containing the WASM bootstrap (or the NOT_BUILT placeholder in CI-without-UI), and `GET /api/jobs` returns JSON. Delete the old `/api/dashboard/feed` assertions in `tests/feed.rs` (fold remaining useful cases into `tests/api_events.rs`).
- [ ] **Step 5:** Remove unused deps from `crates/paavo-web/Cargo.toml` (whatever the dropped code used and nothing else needs). Run the full gate: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Expected: PASS.
- [ ] **Step 6: Commit** — `feat(web): JSON+SSE API + embedded SPA; drop SSR pages/feed`.

---

## Phase 4 — paavo-web-ui (Leptos CSR app)

> All code below targets the Leptos version pinned in Task 0.2 (assumed 0.7). If the spike pinned a different major, adjust reactive primitives (`signal`/`RwSignal`/`Resource::new`/`view!`) per that version's API — the structure and responsibilities are unchanged. Build/preview with `trunk serve --proxy-backend=http://127.0.0.1:8081/api/` (so `/api` hits a running paavo-web). Lint inside the crate dir.

### Task 4.1: App entry, modules, mount

- [ ] Replace `src/main.rs` to mount `App`; create `src/lib.rs`-style module layout: `app.rs` (root + router), `api.rs`, `live.rs`, `theme.rs`, `components/` (`shell.rs`, `jobs_list.rs`, `job_detail.rs`, `dashboard.rs`, `boards.rs`, `schedule.rs`, `widgets.rs`). `main.rs`:
  ```rust
  fn main() { console_error_panic_hook::set_once(); leptos::mount::mount_to_body(paavo_web_ui::App); }
  ```
- [ ] Commit — `feat(web-ui): app module layout + mount`.

### Task 4.2: `api` module (typed fetch wrappers)

- [ ] Implement `api.rs` with `gloo_net::http::Request` calls returning the proto types. Example:
  ```rust
  use paavo_proto::{BoardView, JobListItem, JobView, LogFrame, Page, ScheduleView};
  pub async fn jobs(q: &str, page: u32, as_of: Option<i64>) -> Result<Page<JobListItem>, String> {
      let mut url = format!("/api/jobs?page={page}&per_page=50&q={}", urlencoding(q));
      if let Some(t) = as_of { url.push_str(&format!("&as_of={t}")); }
      gloo_net::http::Request::get(&url).send().await.map_err(e)?.json().await.map_err(e)
  }
  // jobs_one(id) -> JobView; job_log(id, offset) -> Vec<LogFrame>;
  // boards(page) -> Page<BoardView>; schedules(page) -> Page<ScheduleView>.
  ```
  (`e` = `|x| x.to_string()`; `urlencoding` = a tiny percent-encoder or the `urlencoding` crate.)
- [ ] Commit — `feat(web-ui): typed API client`.

### Task 4.3: `live` module (EventSource → revision signals)

- [ ] Implement `live.rs`: open one `web_sys::EventSource("/api/events")`, expose `RwSignal<u64>` for jobs/boards/schedules revisions, updated from `addEventListener("jobs"|"boards"|"schedules")`. Components subscribe by reading the relevant signal inside a `Resource`/`Effect` so a bump triggers a refetch. Provide it via context (`provide_context`).
- [ ] Commit — `feat(web-ui): live revision signals over /api/events`.

### Task 4.4: Semantic CSS + theme toggle

- [ ] Author `style.css`: CSS custom properties under `:root` (light) and `.dark` (dark); semantic classes (`.app`, `.sidebar`, `.topbar`, `.card`, `.stat`, `.table`, `.badge.is-{running,passed,failed,...}`, `.pill`, `.logpane`, `.filter`). **Fluid/responsive:** layout via CSS grid (`grid-template-columns: minmax(0,1fr)`, stat cards `repeat(auto-fit, minmax(12rem,1fr))`), flex, `%`, `clamp()`; a `rem` type scale; a `@media (max-width: 48rem)` breakpoint that collapses the sidebar to a top bar and makes tables horizontally scrollable. **No fixed pixel dimensions.**
- [ ] Implement `theme.rs`: read `localStorage["paavo-theme"]` (fallback `prefers-color-scheme`), apply/remove `.dark` on `document.documentElement`, toggle + persist on the sun/moon button.
- [ ] Commit — `feat(web-ui): semantic responsive CSS + light/dark theme toggle`.

### Task 4.5: Shell + router + nav

- [ ] `app.rs`: `leptos_router` routes `/`, `/jobs`, `/jobs/:id`, `/boards`, `/schedule`, wrapped in `Shell` (sidebar nav with active highlight, topbar with breadcrumb + sun/moon). `provide_context` the live signals + theme.
- [ ] Manual check: `trunk serve`, click nav, routes render placeholders; toggle flips theme + persists across reload.
- [ ] Commit — `feat(web-ui): shell, router, nav, theme wiring`.

### Task 4.6: Jobs list (search + pagination + live + "N new" pill) — canonical component

- [ ] Implement `components/jobs_list.rs`:
  - Signals: `query: RwSignal<String>` (debounced ~150ms via `gloo-timers`), `page: RwSignal<u32>`, `as_of: RwSignal<Option<i64>>`.
  - `Resource` keyed on `(query, page, as_of, jobs_revision)` → `api::jobs(...)`. Reading the live `jobs_revision` signal inside the resource source makes an `/api/events` `jobs` bump refetch the current view (in-place state updates).
  - Render: a search `<input>` bound to `query`; a match count; a `<table class="table">` of rows (full ULID link, `badge is-{state}`, priority, submitter, board, relative time); a pagination footer (`Page<JobListItem>.total`/`per_page` → page numbers); an "↑ N new" pill shown when `page.new_count > 0` (default view), which on click sets `as_of = Some(now_ms)` (re-pin) and `page = 1`.
  - On first load `as_of` is `None`; the first response's `as_of` is echoed and stored so subsequent pages are stable. Starting a new search clears `as_of`.
  - Time formatting: a small `widgets::rel_time(epoch_ms)` ("3m ago") + `title` absolute.
- [ ] Manual check against a seeded DB (Phase 5): typing filters live; pagination works; submitting a job elsewhere bumps the pill; an in-flight job's badge advances in place.
- [ ] Commit — `feat(web-ui): jobs list with fuzzy search, pagination, live updates`.

### Task 4.7: Job detail + live log + per-job filter

- [ ] Implement `components/job_detail.rs`:
  - `Resource` for `api::jobs_one(id)` (header: id pill, `badge is-{state}`, submitter, board, priority, timings; phase banner; outcome card when terminal).
  - Initial log via `api::job_log(id, 0)`; then open `EventSource("/api/jobs/:id/stream?since_seq=<max>")` and append `frame` events reactively into a `RwSignal<Vec<LogFrame>>`, de-duping by `seq` (mirror today's `live-log.js` logic). Handle `phase`/`terminal`/`truncated`/`lagged` events (update banner/outcome/stop).
  - Per-job filter `<input>` → client-side predicate over the loaded frames (case-insensitive substring; optional subsequence), with a "N / M lines" count. Render `<pre class="logpane">` with `[build]`/`[run]` tags (from `target` prefix), level classes, relative `mm:ss.fff` ts. Escape message text (Leptos escapes text nodes by default — do NOT use `inner_html` for messages).
- [ ] Manual check: open a live fake-runner job → frames stream, badge/phase advance, outcome card appears on finish; filter narrows instantly.
- [ ] Commit — `feat(web-ui): job detail with live log + per-job filter`.

### Task 4.8: Dashboard

- [ ] Implement `components/dashboard.rs`: derive live stat cards from `api::jobs(...)` + `api::boards(...)` (Running count, Queue = building+awaiting, Boards healthy `H/total`, 24h pass rate from recent jobs), a compact "Recent activity" list (reuse the jobs-row widget + "N new" pill), and a "Board fleet" health panel. All refetch on the relevant revision bump. Grid layout per the mockup (`repeat(auto-fit, minmax(12rem,1fr))` for cards; 2-col content grid collapsing to 1 on narrow).
- [ ] Commit — `feat(web-ui): live dashboard (stat cards + recent + fleet)`.

### Task 4.9 & 4.10: Boards + Schedule pages

- [ ] `components/boards.rs`: `Resource` on `(page, boards_revision)` → `api::boards(page)`; `<table>` (id, kind, health badge, infra fails, last used, reason); pagination at 20/pp; optional client-side filter `<input>` over the loaded page.
- [ ] `components/schedule.rs`: `Resource` on `(page, schedules_revision)` → `api::schedules(page)`; `<table>` (id, cron, enabled, last triggered, last completed); pagination at 20/pp.
- [ ] Commit (one each) — `feat(web-ui): boards page (paginated, live)` / `feat(web-ui): schedule page (paginated, live)`.

---

## Phase 5 — Build, CI, AGENTS.md, demo seeder

### Task 5.1: `justfile` + end-to-end embed verification

- [ ] Create `justfile`:
  ```make
  # Build the WASM UI into crates/paavo-web-ui/dist (embedded by paavo-web).
  build-ui:
      cd crates/paavo-web-ui && trunk build --release

  # Run paavo-web after building the UI.
  web: build-ui
      cargo run -p paavo-web -- --config sample-paavo.toml

  # Seed the dev DB with fake boards + jobs to stress-test the UI.
  seed-demo jobs="300":
      cargo run --manifest-path dev/seed-demo/Cargo.toml -- \
        --db /tmp/paavo/paavo.sqlite --boards 6 --jobs {{jobs}}
  ```
- [ ] Verify: `just build-ui` then `cargo run -p paavo-web -- --config sample-paavo.toml`; open `http://127.0.0.1:8081` → the SPA loads (not the NOT_BUILT placeholder).
- [ ] Commit — `chore: justfile (build-ui, web, seed-demo)`.

### Task 5.2: CI

- [ ] In the CI workflow, before the `cargo test --workspace` job: add `rustup target add wasm32-unknown-unknown`, install trunk (`cargo install trunk --locked` or a prebuilt action), and run `just build-ui` (so `crates/paavo-web-ui/dist` exists for the embed). Add a separate step that runs `cargo fmt --check` + `cargo clippy -- -D warnings` **inside** `crates/paavo-web-ui`.
- [ ] Commit — `ci: build WASM UI (wasm target + trunk) before workspace tests`.

### Task 5.3: AGENTS.md

- [ ] Update the `paavo-web` crate-map row (now "Leptos CSR SPA: JSON+SSE API over RO sqlite, embeds the wasm UI"), add a `paavo-web-ui` row (excluded; trunk; reuses proto), add the wasm/trunk build step to Commands, and fix the "known doc/code drift" note that says paavo-web is *not* Leptos (it now is, CSR).
- [ ] Commit — `docs(agents): paavo-web is now a Leptos CSR SPA; add UI build step`.

### Task 5.4: `dev/seed-demo` seeder

**Files:** Create `dev/seed-demo/{Cargo.toml,src/main.rs}`; add `"dev/seed-demo"` to root `Cargo.toml` `exclude`.

- [ ] `Cargo.toml`: `[package] name="seed-demo" edition="2021" publish=false`; deps `paavo-db = { path = "../../crates/paavo-db" }`, `paavo-proto = { path = "../../crates/paavo-proto" }`, `clap = { version="4", features=["derive"] }`, `rand = "0.8"`.
- [ ] `src/main.rs`: open the DB RW (`paavo_db::Db::open(&db)`), register `--boards` fake boards (`BoardRow::insert` with `BoardSpec { id: format!("mcxa266-{i:02}"), kind:"mcxa266", probe_selector: ProbeSelector{vid:"1366".into(),pid:"1015".into(),serial:format!("FAKE{i:02}")}, chip_name:"MCXA266".into(), target_name:"thumbv8m.main-none-eabihf".into(), wiring_profile:None, health:Healthy }`), then insert `--jobs` jobs spread across boards + submitters (`alice`/`bob`/`carol`/`cron`) + a realistic state mix:
  - ~60% terminal `Passed` (insert → `transition_submitted_to_building` → `transition_building_to_awaiting_board("/tmp/x.elf")` → `transition_awaiting_to_running(board)` → `finalize(OutcomeRecord{ state:Passed, outcome:JobOutcome::Passed, finished_at_ms })`), each with a handful of `LogFrame::append_batch` lines (varied levels, a `Test OK`).
  - ~10% `Failed` (`JobOutcome::Failed(TerminalOutcome::TestErr{message})`), ~5% `TimedOut`, plus some non-terminal `Submitted`/`AwaitingBoard`/`Building`/`Running` (set board_id for running) so every badge appears. Use `JobRow::insert` with a distinct `submitted_at` (e.g. `now - i*1000`) so ordering + `as_of` work.
  - Trickle the **last** ~20 jobs with a `--trickle-ms 500` delay so a watching paavo-web shows live inserts + the "N new" pill.
  - Print a summary (counts per state) and the URL `http://127.0.0.1:8081`.
- [ ] Verify: `cargo run --manifest-path dev/seed-demo/Cargo.toml -- --db /tmp/paavo/paavo.sqlite --boards 6 --jobs 300`; open paavo-web → 300 jobs paginate, search works, all badges present, trickle shows live.
- [ ] Commit — `feat(dev): seed-demo DB seeder for UI stress-testing`.

### Task 5.5: Demo runbook

- [ ] Add a short `## Demo / manual UI test` section to `crates/paavo-web/README.md` (or `docs/`) with the exact commands (see "Demo commands" at the end of this plan). Commit — `docs(web): demo runbook + commands`.

---

## Self-review notes

- **Spec coverage:** §3.1 CSR (Phase 0/3/4), §3.2 embed (3.8/5.1), §3.3 CSS (4.4), §3.4 server fuzzy + index (3.2/3.4), §3.5 per-job filter (4.7), §3.6 live list + pill (4.6), §3.7 `/api/events` (3.7), §4.x crates/API (Phase 1–4), §6 types (Phase 1; BoardView reused), §7 responsive (4.4), §9 tests (per task), §10 build/CI/AGENTS (5.1–5.3). Demo deliverable (5.4/5.5). All covered.
- **Type consistency:** `JobListItem`/`ScheduleView`/`Page<T>` defined in Phase 1 are used unchanged in Phases 2–4; `BoardView` is the existing proto type; `LiveState`/`JobIndex`/`Revisions` names are consistent across 3.2–3.9 and 4.3.
- **Known parameterization:** Phase 4 Leptos APIs depend on the Task 0.2 version pin (called out at the phase header) — not a placeholder, a spike-resolved constant.

## Execution handoff — see the message accompanying this plan.

---

## Demo commands (what you asked for)

Run from the worktree, after `just build-ui` once:

```bash
# 1. Start the daemon with the fake runner (every real job Passes). DB → /tmp/paavo/paavo.sqlite
PAAVO_FAKE_RUNNER=1 cargo run -p paavod -- --config sample-paavo.toml

# 2. In another shell: start the web UI (serves http://127.0.0.1:8081)
just build-ui            # once, to embed the WASM
cargo run -p paavo-web -- --config sample-paavo.toml

# 3. Register several fake boards (via the CLI → paavod on :8090)
export PAAVO_HOST=http://127.0.0.1:8090
for i in 01 02 03 04 05 06; do
  cargo run -p paavo-cli -- board add \
    --kind mcxa266 --instance mcxa266-$i \
    --probe 1366:1015:FAKE$i --chip MCXA266 \
    --target thumbv8m.main-none-eabihf --wiring-profile default
done

# 4a. Stress-test the UI: seed 300 varied jobs across the boards (volume + variety
#     + live "N new" pill via the trickle). Writes the same /tmp/paavo/paavo.sqlite.
cargo run --manifest-path dev/seed-demo/Cargo.toml -- \
  --db /tmp/paavo/paavo.sqlite --boards 6 --jobs 300 --trickle-ms 500

# 4b. See a genuinely live job stream: submit the smoke fixture to a board and follow it
cargo run -p paavo-cli -- run tests/fixtures/smoke-crate \
  --board-kind mcxa266 --instance mcxa266-01 --follow

# Open http://127.0.0.1:8081 — page through 300 jobs, fuzzy-search "alice mcx",
# watch badges advance live, open a job to tail its log + filter it, toggle dark/light.
```

> Note: paavod's startup reconciliation aborts any seeded `building`/`running` rows if you (re)start paavod against the seeded DB. For the frozen-variety browsing demo, the seeder + paavo-web alone is enough (paavo-web only reads). Use 4b for a truly live in-flight job.
