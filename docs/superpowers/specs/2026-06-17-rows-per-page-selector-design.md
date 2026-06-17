# Rows-per-page selector for paginated lists

**Date:** 2026-06-17
**Status:** Design approved; ready for implementation plan
**Scope:** `paavo-web-ui` (Leptos CSR SPA) + a one-line default change in `paavo-web`

## Problem

The three paginated list pages in the web UI — Jobs (`/jobs`), Boards
(`/boards`), and Schedule (`/schedule`) — render a fixed page size with no way
for the operator to change how many rows appear per page. The jobs list
defaults to 50 rows; boards and schedule default to 20. Operators want to pick
the page size themselves, and the default should be the smaller, faster 20.

## Goals

- Let the user choose the page size on every paginated list page.
- Offer a fixed set of sizes: **10, 20, 30, 40, 50, 100**.
- Default to **20** on all three lists (jobs drops from 50 → 20; boards and
  schedule already default to 20).
- Remember each list's choice across reloads and navigation, **per browser /
  per user**, not server-side.

## Non-goals

- No "all" / unbounded option. It was considered and rejected: the jobs index
  can grow to thousands of rows, and an unbounded response plus a heavy DOM
  render is not worth the convenience. Every offered size (≤ 100) stays within
  the server's existing payload clamps, so no clamp changes are needed.
- No server-side or shared persistence. The choice is a per-user UI preference,
  stored in the browser only — never in SQLite.
- The dashboard's counting fetches (`jobs_page(per_page=200)`,
  `boards_page(per_page=100)`) are **not** list views and are left untouched.
- No URL/query-string state for page size (matches the existing pattern: the
  current `page` number is also a component signal, not a URL parameter).

## Current architecture (as built)

- **Wire envelope:** `paavo-proto::Page<T>` (`crates/paavo-proto/src/page.rs`)
  already carries `per_page`; the server echoes it back and the SPA computes
  `total_pages = total.div_ceil(per_page)`. No wire change is required.
- **Server handlers** (`crates/paavo-web/src/api/`):
  - `jobs::list` — `per_page` default `50`, clamp `1..=200`.
  - `boards::list` — `per_page` default `20`, clamp `1..=100`.
  - `schedules::list` — `per_page` default `20`, clamp `1..=100`.
- **UI API client** (`crates/paavo-web-ui/src/api.rs`): general `*_page(...)`
  functions take an explicit `per_page`; thin wrappers (`jobs`, `boards`,
  `schedules`) hardcode a size (50 / 20 / 20).
- **UI list pages** (`jobs_list.rs`, `boards.rs`, `schedule.rs`): each owns a
  1-based `page: RwSignal<u32>`, keys a `LocalResource` on it, and renders the
  shared `pager(page, current, total_pages)` footer **only when**
  `total_pages > 1`. There is no rows-per-page control today.
- **Existing conventions to mirror:**
  - `crates/paavo-web-ui/src/components/widgets.rs` — home of shared,
    presentation-only view helpers (`pager`, `StateBadge`, …).
  - `crates/paavo-web-ui/src/theme.rs` — the localStorage pattern:
    a `storage() -> Option<web_sys::Storage>` guard
    (`window()?.local_storage().ok().flatten()`, `None` in privacy modes),
    default-on-absent, best-effort persist-on-change. `web-sys` already enables
    the `"Storage"` + `"Window"` features, so **no new dependency** is needed.

## Approach (chosen: shared widget + storage module)

Add one small storage module and one presentation-only widget, then wire the
same four-part pattern into each of the three list components. The `per_page`
signal stays at the component level (where the data resource lives) so a size
change re-keys and refetches the resource correctly. Rejected alternatives: a
fully-encapsulated `PaginationFooter` component (the resources must key on
`per_page`, so the signal can't be hidden inside the footer — awkward prop
plumbing for little gain) and inlining everything per component (triplicates
storage/validation/reset/option-list, fights the `widgets.rs` convention).

### 1. New module `crates/paavo-web-ui/src/per_page.rs`

Single source of truth for the option set and persistence. Mirrors `theme.rs`.

- `pub const OPTIONS: [u32; 6] = [10, 20, 30, 40, 50, 100];`
- `pub const DEFAULT: u32 = 20;`
- Storage-key consts: `paavo-per-page-jobs`, `paavo-per-page-boards`,
  `paavo-per-page-schedule`.
- `pub fn load(key: &str) -> u32` — read `localStorage[key]`, parse, and return
  it **only if it is a member of `OPTIONS`**; otherwise `DEFAULT`. A missing
  key, unavailable storage (privacy mode), non-numeric value, or an
  out-of-set value (stale `999`, an old `50` is still valid) all collapse to a
  safe in-set size.
- `pub fn store(key: &str, n: u32)` — best-effort `set_item`, errors ignored
  (same as `theme::toggle`).
- Register `pub mod per_page;` in `lib.rs` (alongside the existing
  `pub mod theme;`).

Validation on load is the robustness keystone: the table can never be driven
outside the allowed set by stale or hand-edited storage.

### 2. New widget in `widgets.rs`

`pub fn per_page_selector(per_page: RwSignal<u32>) -> impl IntoView`:

- Renders `Rows per page: <select class="per-page">…</select>`.
- Options come from `per_page::OPTIONS`; the `<option>` equal to
  `per_page.get()` is marked `selected`.
- `on:change` parses the selected value and calls `per_page.set(n)`.
- Presentation-only and side-effect-free (no storage, no page reset) — exactly
  like the existing `pager`, so it stays reusable.

### 3. Footer composition (always rendered)

Each component replaces its conditional `{(total_pages > 1).then(|| pager(...))}`
with a footer that **always** renders the selector and keeps the pager buttons
conditional:

```rust
view! {
    <div class="list-footer">
        {per_page_selector(per_page)}
        {(total_pages > 1).then(|| pager(page, cur_page, total_pages))}
    </div>
}
```

So on a single page the dropdown is still visible (and can be shrunk, which may
create a second page); the prev/next/number buttons appear only when
`total_pages > 1`. A small CSS rule (`style.css`) lays selector + pager on one
line (selector left, pager right).

### 4. Per-component wiring (jobs, boards, schedule)

Each component gains the same four parts:

1. `let per_page = RwSignal::new(per_page::load(KEY));` using the list's
   storage-key const.
2. The data `LocalResource` also reads `per_page.get()` and passes it to the
   API call (Section 5).
3. An `Effect` using the existing skip-first-run idiom: when `per_page` changes,
   call `per_page::store(KEY, n)` and `page.set(1)`.
4. The always-rendered footer from Section 3.

For **jobs**, a `per_page` change resets `page → 1` but leaves the `as_of` pin
untouched, so the stable list window and the "↑ N new" pill keep working. This
composes cleanly with the existing query-transition effect — they are separate
effects that both only ever reset `page`.

### 5. API client (`api.rs`)

List components always send an explicit, user-chosen `per_page`:

- **jobs:** call `api::jobs_page(&q, page, per_page, as_of)` (already exists).
- **boards:** call `api::boards_page(page, per_page, &q)` (already exists).
- **schedules:** add `pub async fn schedules_page(page: u32, per_page: u32)`;
  the component calls it.

The hardcoded thin wrappers (`jobs` → 50, `boards` → 20, `schedules` → 20) that
become unused on the list path are removed to avoid dead code — CI runs with
`-D warnings`, so an unused `pub` wouldn't fail, but unused private items would;
remove cleanly regardless. The dashboard keeps calling `jobs_page` /
`boards_page` directly and is unaffected.

### 6. Server side (`paavo-web/src/api/jobs.rs`)

One consistency change: the jobs handler's `per_page` default `unwrap_or(50)`
becomes `unwrap_or(20)` to match the new UI default for direct API callers.
Boards and schedules already default to 20. Clamps are unchanged (every option
≤ 100 fits within `1..=200` and `1..=100`).

## Data flow (after change)

1. Component mounts → `per_page = per_page::load(KEY)` (stored choice, else 20).
2. `LocalResource` keyed on `(dq, page, per_page, as_of?, live-revision)` calls
   the matching `*_page` API with the chosen size.
3. Server clamps (no-op for our options), queries, echoes `per_page` in `Page`.
4. UI renders rows + always-on footer; `total_pages` derived from echoed
   `per_page`.
5. User picks a new size → widget sets `per_page` → Effect persists to
   localStorage and resets `page → 1` → resource refetches.

## Testing

- **`per_page` module** (`wasm-bindgen-test`): `load` returns 20 when the key is
  absent; returns a valid stored `30`; returns 20 for an out-of-set `999`;
  returns 20 for a non-numeric value.
- **Server** (`paavo-web/tests/api_jobs.rs`): a request with no `per_page`
  asserts `per_page == 20` (new default); an explicit `per_page=30` round-trips.
  The existing `per_page=100` clamp test already covers the ceiling.
- **Manual / visual:** each list page shows the dropdown (including on a single
  page); changing size refetches and resets to page 1; reload preserves the
  per-list choice independently (jobs vs boards vs schedule).
- **Golden gate:** `cargo fmt --all`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo test --workspace`, plus `just build-ui` (trunk) to
  confirm the workspace-excluded wasm SPA still compiles.

## Files touched

| File | Change |
|------|--------|
| `crates/paavo-web-ui/src/per_page.rs` | **new** — options, default, keys, `load`/`store` |
| `crates/paavo-web-ui/src/lib.rs` | register `pub mod per_page;` |
| `crates/paavo-web-ui/src/components/widgets.rs` | **new** `per_page_selector` widget |
| `crates/paavo-web-ui/src/components/jobs_list.rs` | per_page signal + effect + footer; call `jobs_page` |
| `crates/paavo-web-ui/src/components/boards.rs` | per_page signal + effect + footer; call `boards_page` |
| `crates/paavo-web-ui/src/components/schedule.rs` | per_page signal + effect + footer; call `schedules_page` |
| `crates/paavo-web-ui/src/api.rs` | add `schedules_page`; drop now-unused hardcoded wrappers |
| `crates/paavo-web-ui/style.css` | `.list-footer` + `.per-page` layout (beside the existing `.pager` rules) |
| `crates/paavo-web/src/api/jobs.rs` | jobs `per_page` default 50 → 20 |
| `crates/paavo-web/tests/api_jobs.rs` | default-20 + explicit-size assertions |
