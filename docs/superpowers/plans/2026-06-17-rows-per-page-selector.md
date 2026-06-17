# Rows-per-page Selector Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let operators choose how many rows each paginated list page (Jobs, Boards, Schedule) shows — from 10/20/30/40/50/100 — defaulting to 20 and remembering each list's choice per-browser.

**Architecture:** Add a small `per_page` storage module (mirroring `theme.rs`) and a presentation-only `per_page_selector` widget (mirroring `pager`) to the Leptos CSR crate `paavo-web-ui`. Each of the three list components owns a `per_page` signal restored from `localStorage`, keys its data resource on it, and renders an always-visible footer (selector + conditional pager). The server's jobs handler default changes 50 → 20; no clamp or wire changes.

**Tech Stack:** Rust 1.95.0, Leptos 0.7.8 (CSR/wasm32), `web-sys` localStorage, `axum` (paavo-web), `wasm-bindgen-test` for pure-logic unit tests.

---

## Context the implementer needs

- **Two crates, two toolchains.** `paavo-web` is a normal workspace crate (host
  target, tested by `cargo test --workspace`). `paavo-web-ui` is
  **workspace-excluded** and compiles to `wasm32-unknown-unknown` — it is NOT
  built or tested by `cargo test --workspace`. CI validates it separately with
  `cargo fmt -- --check`, `cargo clippy --target wasm32-unknown-unknown
  --all-targets -- -D warnings`, and `trunk build --release`.
- **Verifying UI tasks** (run from `crates/paavo-web-ui`):
  ```bash
  cargo fmt -- --check
  cargo clippy --target wasm32-unknown-unknown --all-targets -- -D warnings
  ```
  Prereqs (one-time): `rustup target add wasm32-unknown-unknown`. The crate's
  `.cargo/config.toml` already sets the getrandom wasm backend.
- **`cargo clippy --all-targets` compiles `#[cfg(test)]` modules**, so the
  wasm-bindgen tests in Task 2 are compile-checked by CI even though CI does not
  *run* them. Keep their logic pure so they would also pass under a wasm test
  runner (`wasm-pack test --node`), but their passing is not part of the golden
  gate.
- **localStorage pattern** already in the codebase: see
  `crates/paavo-web-ui/src/theme.rs` — a `storage()` guard returning
  `Option<web_sys::Storage>`, default-on-absent, best-effort persist. `web-sys`
  already enables the `"Storage"` + `"Window"` features (no Cargo change).
- **Reactive footer pattern** already in the codebase: each list component
  renders `{(total_pages > 1).then(|| pager(page, cur_page, total_pages))}`
  inside a `Suspend` body that re-runs when its `LocalResource` resolves.
- **Naming clash to watch:** each component already binds a *local* `let
  per_page = data.per_page.max(1) as u64;` for the `total_pages` math. The new
  *signal* is also named `per_page`. Each wiring task renames the local to
  `per_page_n` to avoid shadowing the signal.

## File structure

| File | Responsibility | Change |
|------|----------------|--------|
| `crates/paavo-web/src/api/jobs.rs` | jobs list handler | default `per_page` 50 → 20 |
| `crates/paavo-web/tests/api_jobs.rs` | jobs API integration tests | add default-20 + explicit-size test |
| `crates/paavo-web-ui/src/per_page.rs` | **new** — options, default, keys, `sanitize`/`load`/`store` | create |
| `crates/paavo-web-ui/src/lib.rs` | crate module list | register `pub mod per_page;` |
| `crates/paavo-web-ui/src/components/widgets.rs` | shared view widgets | add `per_page_selector` |
| `crates/paavo-web-ui/style.css` | SPA stylesheet | add `.list-footer` + `.per-page` |
| `crates/paavo-web-ui/src/components/jobs_list.rs` | `/jobs` page | per_page signal + effect + footer; call `jobs_page` |
| `crates/paavo-web-ui/src/components/boards.rs` | `/boards` page | per_page signal + effect + footer; call `boards_page` |
| `crates/paavo-web-ui/src/api.rs` | typed fetch wrappers | add `schedules_page`; drop unused hardcoded wrappers |
| `crates/paavo-web-ui/src/components/schedule.rs` | `/schedule` page | per_page signal + effect + footer; call `schedules_page` |

---

## Task 1: Server jobs default 50 → 20

**Files:**
- Modify: `crates/paavo-web/src/api/jobs.rs:40`
- Test: `crates/paavo-web/tests/api_jobs.rs` (append a new test)

- [ ] **Step 1: Write the failing test**

Append to `crates/paavo-web/tests/api_jobs.rs` (after the existing
`jobs_list_paginates_and_searches` test; reuses the helpers already in the file):

```rust
#[tokio::test]
async fn jobs_default_page_size_is_20_and_explicit_is_echoed() {
    let (_dir, rw, app) = jobs_app(Duration::from_millis(20));
    JobRow::insert(rw.raw_conn(), &new_job(JobId::new(), "alice"), 0).unwrap();
    wait_for_total(&app, 1, Duration::from_secs(5)).await;

    // No per_page in the query → server falls back to the default page size.
    let p = get_page(&app, "/api/jobs").await;
    assert_eq!(p.per_page, 20, "default jobs page size should be 20");

    // An explicit, in-range per_page is echoed back unchanged.
    let p30 = get_page(&app, "/api/jobs?per_page=30").await;
    assert_eq!(p30.per_page, 30);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p paavo-web --test api_jobs jobs_default_page_size_is_20_and_explicit_is_echoed`
Expected: FAIL — `assert_eq!(p.per_page, 20)` sees `50` (the current default).

- [ ] **Step 3: Change the default**

In `crates/paavo-web/src/api/jobs.rs`, change the `per_page` default from 50 to 20:

```rust
    let per_page: u32 = q
        .get("per_page")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
        .clamp(1, 200);
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p paavo-web --test api_jobs`
Expected: PASS (both the new test and the existing `jobs_list_paginates_and_searches`).

- [ ] **Step 5: Commit**

```bash
git add crates/paavo-web/src/api/jobs.rs crates/paavo-web/tests/api_jobs.rs
git commit -m "feat(web): default jobs list page size to 20"
```

---

## Task 2: `per_page` storage module

**Files:**
- Create: `crates/paavo-web-ui/src/per_page.rs`
- Modify: `crates/paavo-web-ui/src/lib.rs` (add `pub mod per_page;`)

- [ ] **Step 1: Write the module with its failing-to-run tests**

Create `crates/paavo-web-ui/src/per_page.rs`:

```rust
//! Per-list "rows per page" preference: the offered sizes, the default, and
//! browser-local (per-user) persistence.
//!
//! Mirrors [`crate::theme`]'s localStorage pattern: a `storage()` guard that
//! yields `None` in privacy modes, default-on-absent, and best-effort
//! persist-on-change. The chosen size is a UI preference only — it is never
//! sent to or stored by the server.

/// The page sizes offered in the selector, in display order. The selector
/// ([`crate::components::widgets::per_page_selector`]) and the validation in
/// [`sanitize`] both key off this single list.
pub const OPTIONS: [u32; 6] = [10, 20, 30, 40, 50, 100];

/// The page size used when there is no stored choice, storage is unavailable,
/// or a stored value is not a member of [`OPTIONS`].
pub const DEFAULT: u32 = 20;

/// `localStorage` key for the jobs list's page size.
pub const KEY_JOBS: &str = "paavo-per-page-jobs";
/// `localStorage` key for the boards list's page size.
pub const KEY_BOARDS: &str = "paavo-per-page-boards";
/// `localStorage` key for the schedule list's page size.
pub const KEY_SCHEDULE: &str = "paavo-per-page-schedule";

/// Validate a raw stored string against [`OPTIONS`], collapsing anything
/// missing, non-numeric, or out-of-set to [`DEFAULT`]. Pure (no DOM access),
/// so it is unit-testable without a browser.
pub fn sanitize(raw: Option<String>) -> u32 {
    raw.and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|n| OPTIONS.contains(n))
        .unwrap_or(DEFAULT)
}

/// The stored page size for `key`, validated through [`sanitize`]. Returns
/// [`DEFAULT`] when the key is absent or storage is unavailable.
pub fn load(key: &str) -> u32 {
    sanitize(storage().and_then(|s| s.get_item(key).ok().flatten()))
}

/// Persist `n` as the page size for `key`. Best-effort: storage errors and
/// privacy-mode unavailability are silently ignored (matching `theme::toggle`).
pub fn store(key: &str, n: u32) {
    if let Some(s) = storage() {
        let _ = s.set_item(key, &n.to_string());
    }
}

/// `window.localStorage`, if available (absent in some privacy modes).
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn absent_is_default() {
        assert_eq!(sanitize(None), DEFAULT);
    }

    #[wasm_bindgen_test]
    fn valid_in_set_is_kept() {
        assert_eq!(sanitize(Some("30".into())), 30);
        assert_eq!(sanitize(Some("100".into())), 100);
        assert_eq!(sanitize(Some("  50 ".into())), 50);
    }

    #[wasm_bindgen_test]
    fn out_of_set_is_default() {
        assert_eq!(sanitize(Some("999".into())), DEFAULT);
        assert_eq!(sanitize(Some("25".into())), DEFAULT);
        assert_eq!(sanitize(Some("0".into())), DEFAULT);
    }

    #[wasm_bindgen_test]
    fn non_numeric_is_default() {
        assert_eq!(sanitize(Some("lots".into())), DEFAULT);
        assert_eq!(sanitize(Some(String::new())), DEFAULT);
    }
}
```

- [ ] **Step 2: Register the module (and keep the module-list doc current)**

In `crates/paavo-web-ui/src/lib.rs`, add `pub mod per_page;` between
`pub mod live;` and `pub mod theme;`:

```rust
pub mod live;
pub mod per_page;
pub mod theme;
```

Then add a matching bullet to the `//! Module layout:` list in the crate doc
comment (after the `live` bullet, before the `theme` bullet) so the doc stays in
sync (per AGENTS.md "keep docs current"):

```rust
//! - [`per_page`] — per-list rows-per-page preference: offered sizes, default,
//!   and browser-local persistence.
```

- [ ] **Step 3: Verify it compiles clean on wasm (including the test module)**

Run (from `crates/paavo-web-ui`):
```bash
cargo clippy --target wasm32-unknown-unknown --all-targets -- -D warnings
cargo fmt -- --check
```
Expected: no errors, no warnings. (`--all-targets` compiles the `#[cfg(test)]`
module, so a signature or type error in the tests fails here.)

- [ ] **Step 4: Commit**

```bash
git add crates/paavo-web-ui/src/per_page.rs crates/paavo-web-ui/src/lib.rs
git commit -m "feat(web-ui): per-list rows-per-page storage module"
```

---

## Task 3: `per_page_selector` widget + footer CSS

**Files:**
- Modify: `crates/paavo-web-ui/src/components/widgets.rs` (append a function)
- Modify: `crates/paavo-web-ui/style.css` (append rules after `.pager-gap`)

- [ ] **Step 1: Add the widget**

Append to `crates/paavo-web-ui/src/components/widgets.rs` (the file already has
`use leptos::prelude::*;`, which brings in `event_target_value`):

```rust
/// A `<select>` of the offered page sizes ([`crate::per_page::OPTIONS`]),
/// two-way bound to the caller's `per_page` signal. `current` is the size the
/// server echoed for the page on screen, used to pre-select the matching
/// `<option>`.
///
/// Presentation-only: it neither persists the choice nor resets the page — the
/// owning component does both in an `Effect` (see `jobs_list.rs`), mirroring how
/// [`pager`] leaves page-state mutation to its `page` signal. Shared by the
/// jobs, boards, and schedule footers so every list offers identical sizes.
pub fn per_page_selector(per_page: RwSignal<u32>, current: u32) -> impl IntoView {
    let options = crate::per_page::OPTIONS
        .iter()
        .map(|&n| {
            view! {
                <option value=n selected=(n == current)>
                    {n}
                </option>
            }
        })
        .collect::<Vec<_>>();
    view! {
        <label class="per-page">
            "Rows per page: "
            <select on:change=move |ev| {
                if let Ok(n) = event_target_value(&ev).parse::<u32>() {
                    per_page.set(n);
                }
            }>
                {options}
            </select>
        </label>
    }
}
```

- [ ] **Step 2: Add the footer + selector CSS**

In `crates/paavo-web-ui/style.css`, immediately after the `.pager-gap { … }`
rule (around line 448), add:

```css
/* The list footer: rows-per-page selector pinned left, pager pushed right.
 * Always rendered (even on a single page) so the size is always changeable. */
.list-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.75rem;
  flex-wrap: wrap;
  margin-top: 0.75rem;
}

.per-page {
  display: inline-flex;
  align-items: center;
  gap: 0.4rem;
  font-size: 0.875rem;
  color: var(--text-muted);
}

.per-page select {
  padding: 0.3rem 0.5rem;
  border-radius: 0.5rem;
  border: 1px solid var(--border);
  background: var(--surface);
  color: var(--text);
  font: inherit;
  font-size: 0.875rem;
  cursor: pointer;
}
```

- [ ] **Step 3: Verify it compiles clean on wasm**

Run (from `crates/paavo-web-ui`):
```bash
cargo clippy --target wasm32-unknown-unknown --all-targets -- -D warnings
cargo fmt -- --check
```
Expected: no errors, no warnings. (`per_page_selector` is a `pub fn` in a `pub
mod`, so it is exported and does NOT trigger a dead-code warning while still
unused.)

- [ ] **Step 4: Commit**

```bash
git add crates/paavo-web-ui/src/components/widgets.rs crates/paavo-web-ui/style.css
git commit -m "feat(web-ui): rows-per-page selector widget + footer styles"
```

---

## Task 4: Wire the Jobs list

**Files:**
- Modify: `crates/paavo-web-ui/src/components/jobs_list.rs`
- Modify: `crates/paavo-web-ui/src/api.rs` (remove the now-unused `jobs` wrapper)

- [ ] **Step 1: Update imports**

In `crates/paavo-web-ui/src/components/jobs_list.rs`, change the widgets import
(line 38) to add `per_page_selector`, and add the `per_page` module import below
the existing `use crate::api;` (line 37):

```rust
use crate::api;
use crate::components::widgets::{abs_time, pager, per_page_selector, rel_time, StateBadge};
use crate::live::LiveSignals;
use crate::per_page;
```

- [ ] **Step 2: Add the per_page signal**

After `let page = RwSignal::new(1u32);` (line 53), add:

```rust
    // Rows-per-page, restored from this list's browser-local preference.
    let per_page = RwSignal::new(per_page::load(per_page::KEY_JOBS));
```

- [ ] **Step 3: Thread per_page into the data resource**

Replace the `res` resource (lines 61-67) with a version that reads `per_page`
and calls the explicit-size API:

```rust
    let res = LocalResource::new(move || {
        let q = dq.get();
        let p = page.get();
        let pp = per_page.get();
        let a = as_of.get();
        let _ = live.jobs.get();
        async move { api::jobs_page(&q, p, pp, a).await }
    });
```

- [ ] **Step 4: Add the persist-and-reset effect**

Immediately after the existing query-transition `Effect::new(...)` block (it
ends at line 83), add a second effect:

```rust
    // Persist the page-size choice and jump back to page 1 whenever it changes.
    // The closure returns the size so the next run sees it as `prev`; the first
    // run (prev = None) is the mount, where restoring a stored size must NOT
    // reset paging.
    Effect::new(move |prev: Option<u32>| {
        let pp = per_page.get();
        if let Some(old) = prev {
            if old != pp {
                per_page::store(per_page::KEY_JOBS, pp);
                page.set(1);
            }
        }
        pp
    });
```

- [ ] **Step 5: Rename the local `per_page` and capture the echoed size**

Inside the `Ok(data)` arm, replace the totals block (lines 119-123) so the local
no longer shadows the signal and the echoed size is captured for the selector:

```rust
                        let total = data.total;
                        let new_count = data.new_count;
                        let cur_page = data.page;
                        let cur_per_page = data.per_page;
                        let per_page_n = data.per_page.max(1) as u64;
                        let total_pages = total.div_ceil(per_page_n).max(1) as u32;
```

- [ ] **Step 6: Replace the conditional pager with the always-on footer**

Replace the final pager line (line 189,
`{(total_pages > 1).then(|| pager(page, cur_page, total_pages))}`) with:

```rust
                            <div class="list-footer">
                                {per_page_selector(per_page, cur_per_page)}
                                {(total_pages > 1)
                                    .then(|| pager(page, cur_page, total_pages))}
                            </div>
```

- [ ] **Step 7: Remove the now-unused `jobs` API wrapper + fix its doc link**

In `crates/paavo-web-ui/src/api.rs`, delete the `jobs` thin wrapper (lines 48-53,
`pub async fn jobs(...)` "at the default 50-row page size"). Leave `jobs_page`
(used here and by the dashboard) intact.

Then fix the dangling intra-doc link in `jobs_page`'s own doc comment: it
currently reads "The general form backing [`` `jobs` ``]; the dashboard …",
which would point at the just-deleted item. Replace the `jobs_page` doc comment
(lines 28-34) with:

```rust
/// `GET /api/jobs?q=&page=&per_page=&as_of=` — one page of the (optionally
/// fuzzy-filtered) jobs index with an explicit page size. The dashboard calls
/// it with a large window (`per_page=200`, the server's clamp ceiling) to count
/// in-flight jobs accurately — in-flight rows are always the newest, so a
/// recent window captures them all. Blank `q` returns the time-ordered list;
/// `as_of` pins the page to `submitted_at <= as_of` for stable paging.
```

- [ ] **Step 8: Verify it compiles clean on wasm**

Run (from `crates/paavo-web-ui`):
```bash
cargo fmt
cargo clippy --target wasm32-unknown-unknown --all-targets -- -D warnings
```
Expected: no errors, no warnings (`per_page_selector` is now used; `api::jobs`
removed with no remaining callers).

- [ ] **Step 9: Commit**

```bash
git add crates/paavo-web-ui/src/components/jobs_list.rs crates/paavo-web-ui/src/api.rs
git commit -m "feat(web-ui): rows-per-page selector on the jobs list"
```

---

## Task 5: Wire the Boards list

**Files:**
- Modify: `crates/paavo-web-ui/src/components/boards.rs`
- Modify: `crates/paavo-web-ui/src/api.rs` (remove the now-unused `boards` wrapper)

- [ ] **Step 1: Update imports**

In `crates/paavo-web-ui/src/components/boards.rs`, change the widgets import
(line 30) to add `per_page_selector` and add the `per_page` module import below
`use crate::api;` (line 29):

```rust
use crate::api;
use crate::components::widgets::{abs_time, pager, per_page_selector, rel_time, HealthBadge};
use crate::live::LiveSignals;
use crate::per_page;
```

- [ ] **Step 2: Add the per_page signal**

After `let page = RwSignal::new(1u32);` (line 45), add:

```rust
    // Rows-per-page, restored from this list's browser-local preference.
    let per_page = RwSignal::new(per_page::load(per_page::KEY_BOARDS));
```

- [ ] **Step 3: Thread per_page into the data resource**

Replace the `res` resource (lines 50-55) with:

```rust
    let res = LocalResource::new(move || {
        let q = dq.get();
        let p = page.get();
        let pp = per_page.get();
        let _ = live.boards.get();
        async move { api::boards_page(p, pp, &q).await }
    });
```

- [ ] **Step 4: Add the persist-and-reset effect**

Immediately after the existing query-transition `Effect::new(...)` block (it
ends at line 64), add:

```rust
    // Persist the page-size choice and jump back to page 1 whenever it changes.
    // First run (prev = None) is the mount; restoring a stored size must NOT
    // reset paging.
    Effect::new(move |prev: Option<u32>| {
        let pp = per_page.get();
        if let Some(old) = prev {
            if old != pp {
                per_page::store(per_page::KEY_BOARDS, pp);
                page.set(1);
            }
        }
        pp
    });
```

- [ ] **Step 5: Rename the local `per_page` and capture the echoed size**

Inside the `Ok(data)` arm, replace the totals block (lines 100-103) with:

```rust
                        let total = data.total;
                        let cur_page = data.page;
                        let cur_per_page = data.per_page;
                        let per_page_n = data.per_page.max(1) as u64;
                        let total_pages = total.div_ceil(per_page_n).max(1) as u32;
```

- [ ] **Step 6: Replace the conditional pager with the always-on footer**

Replace the final pager line (line 168,
`{(total_pages > 1).then(|| pager(page, cur_page, total_pages))}`) with:

```rust
                            <div class="list-footer">
                                {per_page_selector(per_page, cur_per_page)}
                                {(total_pages > 1)
                                    .then(|| pager(page, cur_page, total_pages))}
                            </div>
```

- [ ] **Step 7: Remove the now-unused `boards` API wrapper + fix its doc link**

In `crates/paavo-web-ui/src/api.rs`, delete the `boards` thin wrapper (lines
85-90, `pub async fn boards(...)` "at the default 20-row page size"). Leave
`boards_page` (used here and by the dashboard) intact.

Then fix the dangling intra-doc link in `boards_page`'s own doc comment (it reads
"The general form backing [`` `boards` ``]; the dashboard …"). Replace the
`boards_page` doc comment (lines 71-76) with:

```rust
/// `GET /api/boards?page=&per_page=&q=` — one page of the (optionally
/// filtered) board fleet with an explicit page size. The dashboard calls it
/// with `per_page=100` (the server's clamp ceiling) and a blank `q` so its
/// "boards healthy" tally covers the whole fleet in one request. A non-blank
/// `q` narrows by an `id`/`kind` substring matched server-side across the
/// *whole* table.
```

- [ ] **Step 8: Verify it compiles clean on wasm**

Run (from `crates/paavo-web-ui`):
```bash
cargo fmt
cargo clippy --target wasm32-unknown-unknown --all-targets -- -D warnings
```
Expected: no errors, no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/paavo-web-ui/src/components/boards.rs crates/paavo-web-ui/src/api.rs
git commit -m "feat(web-ui): rows-per-page selector on the boards list"
```

---

## Task 6: Wire the Schedule list (+ new API function)

**Files:**
- Modify: `crates/paavo-web-ui/src/api.rs` (add `schedules_page`, remove `schedules`)
- Modify: `crates/paavo-web-ui/src/components/schedule.rs`

- [ ] **Step 1: Add `schedules_page`, remove the hardcoded `schedules`**

In `crates/paavo-web-ui/src/api.rs`, replace the `schedules` wrapper (lines
92-95) with an explicit-size version:

```rust
/// `GET /api/schedules?page=&per_page=` — one page of cron schedules at an
/// explicit page size.
pub async fn schedules_page(page: u32, per_page: u32) -> Result<Page<ScheduleView>, String> {
    fetch_json(&format!("/api/schedules?page={page}&per_page={per_page}")).await
}
```

- [ ] **Step 2: Update imports**

In `crates/paavo-web-ui/src/components/schedule.rs`, change the widgets import
(line 12) to add `per_page_selector` and add the `per_page` module import below
`use crate::api;` (line 11):

```rust
use crate::api;
use crate::components::widgets::{abs_time, pager, per_page_selector, rel_time};
use crate::live::LiveSignals;
use crate::per_page;
```

- [ ] **Step 3: Add the per_page signal**

After `let page = RwSignal::new(1u32);` (line 20), add:

```rust
    // Rows-per-page, restored from this list's browser-local preference.
    let per_page = RwSignal::new(per_page::load(per_page::KEY_SCHEDULE));
```

- [ ] **Step 4: Thread per_page into the data resource**

Replace the `res` resource (lines 24-28) with:

```rust
    let res = LocalResource::new(move || {
        let p = page.get();
        let pp = per_page.get();
        let _ = live.schedules.get();
        async move { api::schedules_page(p, pp).await }
    });
```

- [ ] **Step 5: Add the persist-and-reset effect**

Immediately after the `res` resource block, add:

```rust
    // Persist the page-size choice and jump back to page 1 whenever it changes.
    // First run (prev = None) is the mount; restoring a stored size must NOT
    // reset paging.
    Effect::new(move |prev: Option<u32>| {
        let pp = per_page.get();
        if let Some(old) = prev {
            if old != pp {
                per_page::store(per_page::KEY_SCHEDULE, pp);
                page.set(1);
            }
        }
        pp
    });
```

- [ ] **Step 6: Rename the local `per_page` and capture the echoed size**

Inside the `Ok(data)` arm, replace the totals block (lines 42-45) with:

```rust
                        let total = data.total;
                        let cur_page = data.page;
                        let cur_per_page = data.per_page;
                        let per_page_n = data.per_page.max(1) as u64;
                        let total_pages = total.div_ceil(per_page_n).max(1) as u32;
```

- [ ] **Step 7: Replace the conditional pager with the always-on footer**

Replace the final pager line (line 114,
`{(total_pages > 1).then(|| pager(page, cur_page, total_pages))}`) with:

```rust
                            <div class="list-footer">
                                {per_page_selector(per_page, cur_per_page)}
                                {(total_pages > 1)
                                    .then(|| pager(page, cur_page, total_pages))}
                            </div>
```

- [ ] **Step 8: Verify it compiles clean on wasm**

Run (from `crates/paavo-web-ui`):
```bash
cargo fmt
cargo clippy --target wasm32-unknown-unknown --all-targets -- -D warnings
```
Expected: no errors, no warnings (`api::schedules` removed, `schedules_page` now
its sole list caller).

- [ ] **Step 9: Commit**

```bash
git add crates/paavo-web-ui/src/api.rs crates/paavo-web-ui/src/components/schedule.rs
git commit -m "feat(web-ui): rows-per-page selector on the schedule list"
```

---

## Task 7: Full golden-gate verification + UI bundle build

**Files:** none (verification only)

- [ ] **Step 1: Workspace gate (host)**

Run from the repo root:
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Expected: all pass. (Only `paavo-web` changed in the workspace; the new
`api_jobs.rs` test passes.)

- [ ] **Step 2: UI crate gate (wasm)**

Run from `crates/paavo-web-ui`:
```bash
cargo fmt -- --check
cargo clippy --target wasm32-unknown-unknown --all-targets -- -D warnings
```
Expected: all pass.

- [ ] **Step 3: Build the embedded SPA bundle**

Run from the repo root:
```bash
just build-ui
```
Expected: `trunk build --release` succeeds, producing
`crates/paavo-web-ui/dist/`. (Confirms the wired components compile and bundle.)

- [ ] **Step 4: Manual smoke (optional but recommended)**

```bash
just web            # builds UI + serves http://127.0.0.1:8081
# Optionally seed data first in another shell: just seed-demo
```
Verify in the browser, on each of `/jobs`, `/boards`, `/schedule`:
- The "Rows per page" dropdown shows in the footer (even with ≤ one page of rows).
- It defaults to **20** on first visit.
- Changing the size refetches and jumps to page 1.
- Reloading preserves each list's size **independently** (set jobs=50,
  boards=10, schedule=100; reload; each is remembered).

- [ ] **Step 5: Final commit (only if Step 3/4 required a fix)**

```bash
git add -A
git commit -m "chore(web-ui): rows-per-page selector verification fixes"
```

---

## Self-review notes (for the implementer)

- **Spec coverage:** options 10–100 + default 20 (Tasks 1-3), per-list
  localStorage persistence (Task 2 + wiring effects), always-on footer placement
  (Task 3 CSS + wiring Step 7s), no "all"/no server clamp change (Task 1 only
  touches the default), dashboard untouched (`jobs_page`/`boards_page` kept).
- **Type consistency:** `sanitize(Option<String>) -> u32`, `load(&str) -> u32`,
  `store(&str, u32)`, `per_page_selector(RwSignal<u32>, u32)`,
  `schedules_page(u32, u32)`, `jobs_page(&str, u32, u32, Option<i64>)`,
  `boards_page(u32, u32, &str)` are used identically everywhere they appear.
- **Shadowing:** every wiring task renames the local `per_page` → `per_page_n`
  so the `per_page` signal stays in scope for the footer.
- **Commit-green invariant:** Tasks 2 and 3 add exported `pub` items that are
  unused until Task 4+, but `pub` items in a `pub mod` are not dead-code-warned,
  so each task's tree compiles clean under `-D warnings`.
