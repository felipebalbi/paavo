# Design: paavo-web v2 — Leptos CSR single-page app with live updates, fuzzy search, and pagination

**Status**: design approved 2026-06-16. Large change, scoped to **paavo-web only** plus **additive** wire types in `paavo-proto` and **additive** query helpers in `paavo-db`. **No paavod change. No schema migration.** No change to the `WireMessage` / `LogFrame` stream contract.

---

## 1. Goal

Replace paavo-web's server-rendered HTML pages with a **modern, client-rendered WASM single-page app** built on Leptos, while keeping paavo-web a **single binary** that reads the read-only SQLite mirror. Concretely:

- **Event-driven everywhere.** Anything backed by the DB — jobs, boards, schedules, the dashboard — updates on its own as paavo-web observes new data, with no full-page reload.
- **Fuzzy search.** A single input box filters the jobs list as the user types, fzf-style, over the **full** job history.
- **Pagination.** Default 50 jobs, 20 boards, 20 schedules per page, stable under live inserts.
- **A genuinely nice UI.** A sidebar dashboard shell, light/dark toggle, live state badges, a per-job log filter, and a **fluid, responsive** layout that renders beautifully at any resolution.

This supersedes the two existing live surfaces (the per-job log SSE proxy and the dashboard tbody poller) with one coherent client app + API.

## 2. Background: the current shape and the gap

paavo-web today is **plain axum + server-rendered HTML** (`format!` + `include_str!`), not a SPA:

- Each page handler (`pages/dashboard.rs`, `jobs_list.rs`, `job_detail.rs`, `boards.rs`, `schedule.rs`) reads `db::WebDb` (RO SQLite, WAL) at request time and returns an `Html<String>` built from a shared `html_shell`.
- Two bolt-on live surfaces exist:
  - `/api/jobs/:id/stream` (`proxy.rs`) — an SSE proxy that bridges paavod's NDJSON `WireMessage` log stream into browser `EventSource`, consumed by the baked `assets/live-log.js`.
  - `/api/dashboard/feed` (`feed.rs`) — a single background poller over the RO DB that renders the "Recent jobs" `<tbody>` and fans it out over SSE to `assets/dashboard-live.js`.
- The CSS is a hand-baked stylesheet (`assets/style.css`, ef-cyprus light + ef-symbiosis dark) served from the binary. (Note: paavo-web previously pulled UnoCSS from a CDN and deliberately moved to baked CSS for air-gap/third-party-host/FOUC reasons — see `pages/mod.rs`.)

Gaps versus the goal:

1. **No client framework / no WASM.** Every interaction is a full navigation; only two tables push.
2. **No search.** `db.recent_jobs(limit)` / `list_all` only; no filter, no fuzzy.
3. **No pagination.** `jobs_list.rs` caps at `limit` (default 100, max 500) with no paging; boards/schedules render `list_all` unbounded.
4. **Live updates are piecemeal** — only "Recent jobs" and one job's log are live; boards, schedules, the jobs index, and the dashboard summary are static-until-refresh.

Both job ingress paths (HTTP `POST /jobs` and the cron scheduler) terminate in a SQLite write, so **polling the RO DB remains the one observation point that sees every change from every source.** That property — already exploited by the dashboard poller — is the backbone of the new live model.

## 3. Key decisions (with rejected alternatives)

These were settled during design review; rationale and rejected options are recorded so the next agent inherits the "why".

### 3.1 Rendering: CSR single-page app (not SSR + hydration)

paavo-web serves a tiny `index.html` + a baked WASM bundle plus a JSON + SSE API; the browser boots WASM, fetches data, and renders all UI client-side. **No server-side rendering, no hydration, no `cargo-leptos`.**

*Why:* keeps paavo's build/deploy contract intact — plain `cargo` builds the server, a single binary deploys, CI stays close to today. The one advantage of SSR+hydration (instant first paint before WASM loads) is explicitly a non-goal: paavo runs on a LAN, so a ~100–300 ms blank-until-boot is fine.

*Rejected:* **SSR + hydration via `cargo-leptos` + `#[server]` functions.** Canonical Leptos, best first paint, same Rust renders on both sides — but it breaks "CI runs plain cargo" and the single-binary deploy, compiles every component for two targets (host + wasm) with the attendant `cfg` foot-guns, and buys a first-paint win that doesn't matter here.

### 3.2 Packaging: embed the WASM into the single binary

A **new crate `crates/paavo-web-ui`** holds the Leptos CSR app. It is **excluded from the workspace** — exactly like the cross-compiled fixtures already are (`tests/fixtures/smoke-crate`, `dev/spike-fixture-mcxa266`) — because it targets `wasm32-unknown-unknown` and cannot build with the host toolchain. `trunk build` compiles it to a `dist/` (wasm + JS shim + `index.html` + CSS). paavo-web embeds `dist/` via `rust-embed` and serves it. At runtime it is still **one binary**.

*Why:* preserves the single-binary deploy AGENTS.md values; reuses the existing "excluded cross-compiled crate" precedent so `cargo test --workspace` is untouched; the UI reuses `paavo-proto` wire types so the contract stays single-sourced.

*Rejected:* **ship a `web-dist/` dir alongside the binary** (serve from filesystem) — simpler build, but deploy becomes "binary + asset dir" and the systemd/deploy docs change; gives up single-binary. **`build.rs` shells out to trunk** so a plain `cargo build -p paavo-web` produces everything — self-contained command but fragile (cargo-invoking-cargo, assumes the wasm toolchain on every host build, slow/duplicate builds).

### 3.3 Styling: hand-rolled semantic CSS (no utility engine, no Node)

A small, hand-authored stylesheet: CSS **custom properties** for the two palettes (light/dark) + semantic component classes (`.nav`, `.card`, `.badge.is-running`, `.table`, `.pill`, `.logpane`, …), embedded into the binary. Dark mode is a `.dark` class on `<html>`, toggled by the sun/moon control and persisted in `localStorage`, defaulting to `prefers-color-scheme`.

*Why:* genuinely the simplest path, pure Rust, no new toolchain, and the most aligned with paavo's ethos. We investigated the utility-CSS options the user asked about and found a constraint conflict — **UnoCSS + not-Tailwind + no-Node cannot all hold** with off-the-shelf tools:
- Real UnoCSS generation is **JS-only** (`@unocss/cli`, Node).
- The only pure-Rust generator is **`encre-css`**, which is **Tailwind-compatible** (rejected: the user wants to avoid Tailwind's vocabulary).
- **`unocss-classes`** is an **authoring macro only** (builds class strings, expands variant groups) — it generates **no CSS**, so it can't be the solution by itself.

*Rejected:* **real UnoCSS via the JS engine with a minimal preset** (faithful but reintroduces Node into the UI build/CI); **`encre-css`** (pure-Rust, no Node, but Tailwind-style — vetoed); **`unocss-classes` + hand-authored matching utilities** (means hand-maintaining a mini utility framework, more work than semantic classes for little gain). With the semantic-CSS path, `unocss-classes` adds little and is **dropped**.

### 3.4 Fuzzy search: server-side fuzzy over the full history (in-memory index)

paavo-web maintains a **lightweight in-memory index** of every job's searchable fields, refreshed by the same background poller that drives live updates. Each debounced keystroke hits `GET /api/jobs?q=…&page=…`; the server runs a real fzf-style fuzzy matcher (`nucleo-matcher` or `fuzzy-matcher`) over the whole fleet and returns ranked, paginated matches.

*Why:* satisfies **both** "true fuzzy, filters as you type" **and** "covers the full history once there are thousands of jobs", while keeping the client thin and pagination authoritative on the server. The searchable fields per job are tiny (id, submitter, state, board) — even 100 k jobs is a few MB in RAM and a sub-millisecond match; the LAN round-trip per keystroke is negligible.

*Rejected:* **client-side fuzzy over a recent window** — instant and true-fuzzy, but search is bounded to the loaded window; older jobs need an explicit "load more", which fights the "full history" requirement. **Server-side substring (SQL `LIKE`)** — simplest, full history, but not subsequence/fuzzy (`almcx` would not match `alice … mcxa266`).

### 3.5 Log search scope: jobs-list fuzzy + a per-job client-side log filter

The global fuzzy box filters the **jobs list** only. Inside a single job's log view, a separate instant **client-side** filter narrows that job's frames as you type — bounded to the frames already loaded for that one job (no new server work, no schema change).

*Rejected:* **global full-text over all log contents** — needs an FTS5 index over `log_frame` (potentially millions of rows), a `paavo-db` migration, and write-side index maintenance in **paavod**. Heavy; explicitly out of scope.

### 3.6 Live list behavior: in-place state updates + an "N new" pill

Any row currently visible updates its state live on whatever page you're on (watch `Submitted → Building → Running → Passed` in place). New jobs do **not** reflow your view — instead an "↑ N new" pill appears on the newest, unfiltered page; clicking it pulls them in. Searching/paginating yields a stable result set whose row states still update live.

*Why:* the calm, modern pattern (GitHub Actions / Twitter). Pagination stays stable under inserts via an **`as_of` snapshot pin** (below), and live state transitions never reorder rows because the sort key (`submitted_at`) is immutable across a job's lifecycle.

*Rejected:* **fully-live first page (auto-prepend)** — most aggressively live, but reflows page 1 while you're reading it. **Manual refresh banner** — most stable, least live; contradicts "event-driven".

### 3.7 Live transport: one consolidated SSE channel of revision bumps

A single `GET /api/events` SSE stream carries lightweight **per-resource revision bumps** (`jobs`, `boards`, `schedules`). On a bump, the client refetches its **current view** through the JSON API; visible rows re-render, the "N new" count updates. The existing per-job log proxy (`/api/jobs/:id/stream`) is **kept** for high-frequency live frames.

*Why:* dead-simple and correct at this scale — the server stays stateless per connection, the client holds no merge/dedup/order logic (refetch of ≤50 small rows on a LAN is trivial), and one poller drives every resource. The old `/api/dashboard/feed` pushed pre-rendered `<tbody>` HTML because the **server** rendered rows; now the **client** renders, so pushing a revision + refetching is the natural analogue.

*Rejected:* **per-resource SSE endpoints** (more connections, no real benefit); **rich push of changed rows/JSON deltas** (reintroduces client-side merge logic — the exact drift the old `live-log.js` had to guard against).

## 4. Architecture

### 4.1 Component / data flow

```
                          paavo-web process (one axum binary)
 ┌───────────────────────────────────────────────────────────────────────┐
 │  poller task (1/process)        in-memory state (parking_lot RwLock)    │
 │   every POLL_INTERVAL:          ┌───────────────────────────────────┐   │
 │     read RO sqlite  ───────────▶│ JobIndex: Vec<JobListItem>+haystack│   │
 │     recompute revisions ───────▶│ revisions { jobs, boards, schedule}│   │
 │     bump on change ─────────────└───────────────────────────────────┘   │
 │            │ bump                          ▲ read (short lock, drop      │
 │            ▼                               │ before await)               │
 │     /api/events (SSE) ◀───────────┐   ┌────┴───────────────────────────┐ │
 │            │                      │   │ JSON API handlers              │ │
 │            │ event: jobs/boards/  │   │  /api/jobs?q&page&as_of        │ │
 │            │        schedules     │   │  /api/jobs/:id  /…/log         │ │
 │            ▼                      │   │  /api/boards   /api/schedules  │ │
 └────────────┼──────────────────────┼───┴────────────────────────────────┘ │
              │  (revision bump)     │  (refetch current view)              │
 ┌────────────┼──────────────────────┼──────────────────────────────────────┐
 │  browser   ▼                      ▼     Leptos CSR app (WASM, embedded)    │
 │   EventSource ─▶ signal "jobs rev N" ─▶ Resource refetch ─▶ DOM re-render  │
 │   /api/jobs/:id/stream (existing proxy) ─▶ live log frames                 │
 └───────────────────────────────────────────────────────────────────────────┘
                         paavod  ◀── (unchanged) ── /jobs/:id/stream proxy only
```

The poller is sync DB work; API handlers take a **short** `RwLock` read, clone the page-sized result, and **drop the guard before any `.await`** (consistent with the no-lock-across-await discipline; paavo-web is not `deny`-gated like paavod but follows the same rule).

### 4.2 New crate: `crates/paavo-web-ui` (Leptos CSR)

- **Excluded** from `[workspace] members` (add to `exclude` in the root `Cargo.toml` with a comment mirroring the fixtures' rationale). Targets `wasm32-unknown-unknown`.
- Dependencies: `leptos` (with the `csr` feature), `leptos_router`, `gloo-net` (HTTP fetch + SSE/`EventSource`), `paavo-proto` (wire types), `serde` / `serde_json`, a small logging shim (`console_error_panic_hook`, `wasm-bindgen`).
- **ulid-on-wasm integration point.** `paavo-proto` depends on `ulid`, which pulls `getrandom`; on `wasm32-unknown-unknown` that needs the `js` backend to link. The UI never *generates* ULIDs (only parses/Displays them), so the resolution is either: add `getrandom = { version = "0.2", features = ["js"] }` to the UI crate (feature-unification), or depend on `ulid` with `default-features = false`. The implementation spike confirms which is cleanest; documented here so it isn't a surprise.
- App structure: a root `App` with `leptos_router` routes `/`, `/jobs`, `/jobs/:id`, `/boards`, `/schedule`; a `Shell` component (sidebar nav + topbar + theme toggle); feature components `Dashboard`, `JobsList`, `JobDetail`, `Boards`, `Schedule`; shared `api` module (typed fetch wrappers) and `live` module (one `EventSource` to `/api/events` exposing reactive `revision` signals).

### 4.3 Server (`crates/paavo-web`)

- **Removed:** `pages/` (all server-rendered HTML), `assets/style.css`, `assets/live-log.js`, `assets/dashboard-live.js`, the `/api/dashboard/feed` feed (`feed.rs`), and the dashboard tbody renderer.
- **Kept:** `proxy.rs` (`/api/jobs/:id/stream` log proxy to paavod — now consumed by the Leptos detail view), `config.rs`, `db.rs` (extended), `time.rs` (server-side timestamp helpers still used to format API fields, or move formatting client-side — see §6.4).
- **New:**
  - `embed.rs` — `rust-embed` of `../paavo-web-ui/dist`; a handler serving embedded assets with content-type + cache headers, and an SPA fallback that returns `index.html` for unknown non-`/api` paths. If `dist/` is absent/empty (UI not built), serve a clear "UI not built — run `just build-ui`" placeholder instead of a blank 404.
  - `index.rs` — the in-memory `JobIndex` + `Revisions` types, the generalized poller, and a thin search/paginate function.
  - `api/` — JSON handlers (`jobs.rs`, `boards.rs`, `schedules.rs`) and `events.rs` (consolidated SSE).

### 4.4 Live model details

- **Revisions.** A `Revisions { jobs: u64, boards: u64, schedules: u64 }` behind the lock. Each poll tick recomputes a cheap content fingerprint per resource (e.g., a hash of the ordered `(id, state, …)` tuples, or `MAX(rowid)+row count+state digest`); a changed fingerprint bumps the counter. Bumps are published on a `tokio::sync::watch` (latest-wins, like today's feed) drained by every `/api/events` connection.
- **`/api/events`.** Emits one named SSE event per changed resource (`event: jobs` / `boards` / `schedules`, `data: {"revision":N}`), plus an immediate snapshot on connect (so a reconnect re-syncs with no `Last-Event-ID`), 15 s keep-alive — matching the per-job proxy.
- **Pagination stability (`as_of`).** The jobs list is ordered `submitted_at DESC, id DESC`. The client holds an `as_of` cursor (epoch-ms) for the browsing session; page requests pass it, and the server returns only jobs with `submitted_at <= as_of`, so inserts never shift older pages. `new_count` = jobs newer than `as_of`. Clicking the "N new" pill (or starting a new search) re-pins `as_of = now` and returns to page 1. Boards/schedules are small and change rarely — plain offset pagination, no `as_of`.

## 5. Server API contract

All responses are JSON unless noted. Errors are `(StatusCode, plain text)` as today.

| Method & path | Query | Response |
|---|---|---|
| `GET /` , `GET /assets/*` | — | embedded `index.html` / wasm / js / css (SPA fallback to `index.html`) |
| `GET /api/jobs` | `q`, `page` (1-based), `per_page` (default 50, ≤200), `as_of` (epoch-ms, optional) | `Page<JobListItem>` |
| `GET /api/jobs/:id` | — | `JobView` (404 if unknown) |
| `GET /api/jobs/:id/log` | `offset`, `limit` (default 1000) | `Vec<LogFrame>` |
| `GET /api/boards` | `page`, `per_page` (default 20) | `Page<BoardView>` |
| `GET /api/schedules` | `page`, `per_page` (default 20) | `Page<ScheduleView>` |
| `GET /api/events` | — | SSE: `jobs` / `boards` / `schedules` revision events |
| `GET /api/jobs/:id/stream` | `since_seq` | **existing** SSE log proxy to paavod (unchanged) |

- With `q` present, `/api/jobs` ignores `as_of`-pinning for ranking and returns fuzzy-ranked matches over the whole index (score desc, then `submitted_at DESC` tiebreak), paginated; `new_count` is `0` in search mode (the pill is a default-view affordance).
- `per_page` is clamped server-side; out-of-range `page` returns an empty `items` with the correct `total`.

## 6. Data types

### 6.1 `paavo-proto` additions (additive, `deny_unknown_fields` where it already applies)

```rust
/// Lightweight jobs-list row. Subset of JobView; what the index holds
/// and the list renders. Searchable haystack = id + submitter + state + board.
pub struct JobListItem {
    pub id: JobId,
    pub state: JobState,
    pub priority: Priority,
    pub submitter: String,
    pub board_id: Option<String>,
    pub submitted_at: i64, // epoch-ms
}

/// Wire view of a board (mirrors BoardRow minus nothing server-local).
pub struct BoardView {
    pub spec: BoardSpec,                       // id, kind, health, chip/target, wiring
    pub quarantine_reason: Option<String>,
    pub consecutive_infra_failures: u32,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
}

/// Wire view of a schedule row (mirrors ScheduleRow's public fields).
pub struct ScheduleView { /* id, cron, enabled, last_triggered_at, last_completed_at */ }

/// Generic page envelope.
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
    pub revision: u64,    // resource revision at query time
    pub new_count: u64,   // jobs newer than as_of (0 for boards/schedules/search)
    pub as_of: Option<i64>, // echoed jobs cursor (None for boards/schedules)
}
```

These are pure data (serde) and compile to wasm. Serde round-trip + byte-stability tests live in `paavo-proto/tests` per house style.

### 6.2 `paavo-db` additions (additive)

- `JobRow::list_page(conn, as_of: Option<i64>, offset, limit) -> Vec<JobRow>` and `JobRow::count(as_of: Option<i64>) -> u64`.
- A lightweight `JobRow::list_index(conn) -> Vec<JobListItem>` (selects only the index columns) feeding the in-memory index.
- `BoardRow::list_page(offset, limit)` + `BoardRow::count()`; `ScheduleRow::list_page(offset, limit)` + `ScheduleRow::count()`.
- `WebDb` (in paavo-web `db.rs`) grows thin wrappers for the above.

### 6.3 Search & index (`paavo-web/index.rs`)

- `JobIndex { items: Vec<JobListItem>, haystacks: Vec<String> }` rebuilt each poll tick from `list_index`. `haystack[i]` = lowercased `"{id} {submitter} {state} {board}"`.
- `search(q, page, per_page) -> (Vec<JobListItem>, total)` — if `q` blank, slice by `as_of`+offset; else fuzzy-score each haystack, keep matches, sort by score, paginate.

### 6.4 Timestamp formatting

Relative ("3 minutes ago") + absolute tooltips move **client-side** (the UI is reactive and re-renders "ago" for free on each tick). The API returns raw epoch-ms; the WASM app formats with a small time module (mirrors today's `time.rs` helpers). `time.rs` on the server is retained only if a handler still needs it; otherwise removed.

## 7. Responsive & theming (first-class requirement)

- **No fixed pixel dimensions.** Layout uses CSS **grid** (`grid-template-columns: repeat(auto-fit, minmax(…, 1fr))` for stat cards; `fr` tracks for the content/sidebar split), **flexbox**, `%`, and `clamp()` / `min()` / `max()` for fluid sizing; a `rem`-based type scale that honors the user's root font size.
- **Breakpoints in `rem`/`em`** (not hardcoded device px). The sidebar collapses to a top bar / hamburger below a narrow threshold; the stat-card grid reflows from 4→2→1 columns; data tables become horizontally scrollable (or drop low-priority columns) on narrow viewports.
- **Theming.** Two palettes as CSS custom properties under `:root` and `.dark`; the sun/moon toggle flips the `.dark` class and persists to `localStorage`; initial theme follows `prefers-color-scheme`. Respect `prefers-reduced-motion` for the pulsing "running" indicator.
- Verified by eye at a range of widths (e.g., ~320 px phone → ultrawide) during the manual smoke.

## 8. Failure modes & edge cases

- **UI not built / `dist` empty:** embed handler serves a clear placeholder page (not a blank 404).
- **SSR→connect race / reconnect:** `/api/events` sends an immediate snapshot on connect, so a fresh or reconnected client re-syncs; revision-driven refetch is idempotent.
- **Poller DB error:** log `warn`, keep the last good index + revisions, skip the tick — lists never blank on a transient WAL hiccup (same posture as today's feed).
- **paavod down:** only `/api/jobs/:id/stream` (live log) is affected — it already surfaces 502; the rest of the app (jobs/boards/schedules/dashboard) reads RO SQLite and stays fully functional.
- **`as_of` paging past the end:** empty `items`, correct `total`.
- **Search with many matches:** ranked + paginated; the index match is bounded and fast.
- **Job id in URL invalid / unknown:** detail view shows a typed "invalid id" / "not found" state (mirrors current behavior).
- **XSS:** log messages and submitter text are user/device-controlled; Leptos escapes text nodes by default — no `innerHTML` of untrusted strings (a correctness win over the old `innerHTML` swaps).

## 9. Testing strategy

- **`paavo-proto`** — serde round-trip + byte-stability for `JobListItem`, `BoardView`, `ScheduleView`, `Page<T>`.
- **`paavo-db`** — `list_page` / `count` / `list_index` pagination + `as_of` filtering over a temp DB (RW seed → RO read through WAL, as today).
- **`paavo-web` (server)** — using the existing temp-sqlite + `build_router` harness:
  - `/api/jobs` pagination, `as_of` stability under an inserted row, `new_count`, and fuzzy ranking (`q` matches expected ids in score order).
  - `/api/boards` + `/api/schedules` pagination.
  - `/api/events` emits an immediate snapshot and a `jobs` revision bump after an insert (bounded timeout), mirroring today's `tests/feed.rs`.
  - embed handler serves `index.html` for an unknown client route and a JS content-type for assets.
- **`paavo-web-ui`** — a few `wasm-bindgen-test` unit tests for pure logic (the per-job log filter predicate, theme-toggle persistence, time formatting). Component/E2E coverage is a **manual smoke checklist**, not a headless-browser harness (consistent with how the current JS DOM swaps are tested).
- Existing `tests/smoke.rs` (asserts SSR HTML) and `tests/feed.rs` (asserts `/api/dashboard/feed`) are rewritten against the new API; `tests/proxy.rs` (log proxy) stays.

## 10. Build, CI, and AGENTS.md changes

- **Build:** add `crates/paavo-web-ui` with a `Trunk.toml` + `index.html`. A `just build-ui` (or `xtask`) runs `trunk build --release` → `crates/paavo-web-ui/dist`, which `paavo-web` embeds. Document the prerequisites: `rustup target add wasm32-unknown-unknown` + `cargo install trunk`.
- **CI:** add a step (before building/testing paavo-web) that installs the wasm target + trunk and runs `just build-ui`, so the embedded assets exist. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` are otherwise unchanged (the UI crate is excluded from the workspace; lint/format it separately in its own directory).
- **AGENTS.md:** update the paavo-web row of the crate map (no longer "plain axum + server-rendered HTML"), add `paavo-web-ui` and the wasm/trunk build step, and fix the "known doc/code drift" note that currently says paavo-web is *not* Leptos. Per the golden rules, this lands in the same change.

## 11. Definition of done

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all green; the UI crate builds clean via `just build-ui` and is fmt/clippy-clean in its own directory.
- `cargo run -p paavo-web -- --config sample-paavo.toml` serves the SPA; with `PAAVO_FAKE_RUNNER=1 paavod` running, submitting jobs from another shell shows them appear/advance live (in-place states + "N new" pill), fuzzy search filters the full history as you type, pagination works at 50/20/20, the per-job log streams + filters, and the sun/moon toggle flips light/dark.
- Layout verified fluid/responsive across a range of widths; no fixed-pixel dimensions in the stylesheet.

## 12. Scope — explicitly out

- **Global full-text search over log contents** (FTS5 + paavod write-side index).
- **Any paavod change** (the only paavod dependency is the existing log stream proxy).
- **Auth / multi-tenant** — paavo-web stays an unauthenticated read-only viewer.
- **Configurable poll interval via `paavo.toml`** — hardcoded const, promote later if needed.
- **Mutations from the UI** (cancel/retry/quarantine) — read-only viewer unchanged.
- **SSR/hydration, `cargo-leptos`, and any utility-CSS engine** (§3.1, §3.3).

## 13. Files touched (indicative)

| File | Change |
|---|---|
| `Cargo.toml` (root) | add `crates/paavo-web-ui` to `exclude` (with comment) |
| `crates/paavo-web-ui/**` | **New.** Leptos CSR app, `Trunk.toml`, `index.html`, semantic CSS, components, `api`/`live` modules |
| `crates/paavo-proto/src/{job,board,schedule}.rs` | **New** `JobListItem`, `BoardView`, `ScheduleView`, `Page<T>` + tests |
| `crates/paavo-db/src/{job,board,schedule}.rs` | **New** `list_page` / `count` / `list_index` helpers + tests |
| `crates/paavo-web/src/app.rs` | route table: JSON API + `/api/events` + embed/SPA fallback; drop page routes + `/api/dashboard/feed` + static JS/CSS |
| `crates/paavo-web/src/index.rs` | **New.** `JobIndex`, `Revisions`, generalized poller, search/paginate |
| `crates/paavo-web/src/api/{jobs,boards,schedules,events}.rs` | **New.** JSON + SSE handlers |
| `crates/paavo-web/src/embed.rs` | **New.** `rust-embed` asset serving + SPA fallback + not-built placeholder |
| `crates/paavo-web/src/db.rs` | wrappers for the new paginated/index queries |
| `crates/paavo-web/src/{pages,feed}.rs`, `assets/*` | **Removed** (SSR pages, dashboard feed, baked JS/CSS) |
| `crates/paavo-web/src/proxy.rs` | unchanged (kept) |
| `crates/paavo-web/Cargo.toml` | add `rust-embed`, fuzzy matcher; drop unused deps |
| `crates/paavo-web/tests/*` | rewrite `smoke`/`feed` against the new API; keep `proxy` |
| `.github/workflows/*` (CI) | add wasm target + trunk + `just build-ui` step |
| `justfile` / `xtask` | `build-ui` recipe |
| `AGENTS.md` | crate map + build step + drift note updates |

## 14. Open risks / spikes for the implementation plan

1. **ulid-on-wasm** (§4.2) — confirm `getrandom/js` vs `ulid default-features = false`; ~15-minute spike before committing the UI crate skeleton.
2. **Leptos version pin** — pick a current `0.x` line that builds on Rust 1.95 (CSR feature) and pin it; confirm `leptos_router` API shape.
3. **`rust-embed` folder bootstrapping** — `dist/` must exist at paavo-web compile time; the not-built placeholder + CI ordering cover this, but verify a clean `cargo build -p paavo-web` (no UI built) still compiles and runs with the placeholder.
4. **Fuzzy matcher choice** — `nucleo-matcher` vs `fuzzy-matcher`; pick on API ergonomics + ranking quality during the search handler task.

**Estimated size:** large — a new wasm UI crate plus a server rewrite. Best landed as a series of small scoped commits: proto view types → db pagination → server index+API+events → embed/build wiring → UI shell+theme → UI jobs/search/pagination → UI detail/log → dashboard/boards/schedule → CI/AGENTS.
