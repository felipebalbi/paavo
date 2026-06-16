# paavo-web Live Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the paavo-web dashboard "Recent jobs" table update on its own (new jobs + state transitions) via a server push over SSE, no manual refresh.

**Architecture:** One background poller in paavo-web reads its read-only SQLite every ~1 s, renders the "Recent jobs" `<tbody>` (shared with the SSR page render), and publishes it through a `tokio::sync::watch` channel. A new `GET /api/dashboard/feed` SSE endpoint streams the latest snapshot to every connected browser; a small baked `dashboard-live.js` swaps the table body in place. paavo-web stays read-only; no paavod changes.

**Tech Stack:** Rust, axum 0.7 (SSE via `axum::response::sse`), `tokio::sync::watch`, `async-stream`, rusqlite (WAL + RO), vanilla JS `EventSource`.

**Spec:** `docs/superpowers/specs/2026-06-16-paavo-web-live-dashboard-design.md`

**Execution note:** This plan is implemented in a dedicated git worktree (set up at execution start via the using-git-worktrees skill). All paths below are relative to the repo root.

**Per-task verification commands** (run from repo root; `paavo-web` is the only crate touched):

```
cargo test -p paavo-web
cargo fmt -p paavo-web
cargo clippy -p paavo-web --all-targets -- -D warnings
```

---

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/paavo-web/src/pages/dashboard.rs` | SSR dashboard render; **owns** the shared `recent_jobs_tbody` renderer + `RECENT_JOBS_LIMIT`; carries the live-region ids and the client `<script>` tag |
| `crates/paavo-web/src/feed.rs` | **New.** `JobFeed` (watch wrapper), `EMPTY_PAYLOAD`, `render_payload`, `spawn_poller`, `dashboard_feed` SSE handler |
| `crates/paavo-web/src/lib.rs` | Declare `pub mod feed;` |
| `crates/paavo-web/src/proxy.rs` | `AppState` gains a `feed: JobFeed` field |
| `crates/paavo-web/src/app.rs` | `FromRef<AppState> for JobFeed`; routes `/api/dashboard/feed` + `/static/dashboard-live.js`; `serve_dashboard_live_js` |
| `crates/paavo-web/src/main.rs` | Build the feed, spawn the poller, put the feed on state |
| `crates/paavo-web/src/assets/dashboard-live.js` | **New.** `EventSource` consumer that swaps the `<tbody>` + count |
| `crates/paavo-web/tests/feed.rs` | **New.** SSE integration tests (initial snapshot; push-on-insert) |
| `crates/paavo-web/tests/smoke.rs` | Fix `AppState` constructors; assert dashboard ids + script tag + asset route |
| `crates/paavo-web/tests/proxy.rs` | Fix `AppState` constructor |

---

## Task 1: Extract the shared `recent_jobs_tbody` renderer + live-region ids

**Files:**
- Modify: `crates/paavo-web/src/pages/dashboard.rs`

This is a pure refactor of the existing "Recent jobs" rendering into a reusable function the live feed will call, plus the DOM ids the client JS targets. No feed wiring yet.

- [ ] **Step 1: Write the failing unit test**

Append this test module to the end of `crates/paavo-web/src/pages/dashboard.rs`:

```rust
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

    #[test]
    fn recent_jobs_tbody_empty_renders_placeholder() {
        let html = recent_jobs_tbody(&[], 0);
        assert!(html.contains("no jobs yet"), "got: {html}");
    }

    #[test]
    fn recent_jobs_tbody_renders_a_job_row() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paavo.sqlite");
        let id = JobId::new();
        {
            let db = Db::open(&path).unwrap();
            paavo_db::JobRow::insert(db.raw_conn(), &sample_new_job(id), 0).unwrap();
        }
        let webdb = crate::db::WebDb::open(&path).unwrap();
        let jobs = webdb.recent_jobs(RECENT_JOBS_LIMIT).unwrap();
        let html = recent_jobs_tbody(&jobs, 0);
        assert!(html.contains(&id.to_string()), "row missing job id; got: {html}");
        assert!(html.contains("s-submitted"), "row missing state class; got: {html}");
        assert!(html.contains("alice"), "row missing submitter; got: {html}");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p paavo-web --lib recent_jobs_tbody`
Expected: FAIL to compile — `cannot find function recent_jobs_tbody` / `cannot find value RECENT_JOBS_LIMIT`.

- [ ] **Step 3: Add the const + the extracted renderer**

In `crates/paavo-web/src/pages/dashboard.rs`, just below the imports (before `pub async fn render`), add:

```rust
/// Cap on rows shown in the "Recent jobs" table. Shared by the SSR
/// dashboard render and the live feed so their row sets + counts match.
pub(crate) const RECENT_JOBS_LIMIT: u32 = 20;

/// Cargo package version, for the cache-bust on `/static/dashboard-live.js`.
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Render the inner HTML of the "Recent jobs" `<tbody>` — the `<tr>`
/// rows, or the single `no jobs yet` empty-state row. Single source of
/// truth for row markup, escaping, state classes, and relative
/// timestamps; called by both `render` (SSR) and the live feed.
pub(crate) fn recent_jobs_tbody(jobs: &[paavo_db::JobRow], now_ms: i64) -> String {
    if jobs.is_empty() {
        return r#"<tr><td class="empty" colspan="5">no jobs yet</td></tr>"#.to_string();
    }
    let mut out = String::new();
    for j in jobs {
        // submitted_at is epoch ms (i64); see paavo-db's JobRow.
        let ts_abs = crate::time::epoch_ms_to_utc(Some(j.submitted_at));
        let ts_rel = relative_to_now(j.submitted_at, now_ms);
        out.push_str(&format!(
            r#"<tr><td><a href="/jobs/{id}">{id}</a></td><td class="{sc}">{s:?}</td><td>{p:?}</td><td>{u}</td><td class="ts" title="{ts_abs}">{ts_rel}</td></tr>"#,
            id = j.id,
            sc = super::state_class(j.state),
            s = j.state,
            p = j.priority,
            u = super::html_escape(&j.submitter),
        ));
    }
    out
}
```

- [ ] **Step 4: Rewire `render` to use them (and add the ids + script tag)**

In `pub async fn render`, make four edits:

1. Change the jobs query to use the shared cap:

```rust
    let jobs = db.recent_jobs(RECENT_JOBS_LIMIT).unwrap_or_default();
```

2. Add the id to the recent-jobs count `<strong>`:

```rust
    body.push_str(&format!(
        r#"<p class="muted"><strong>{}</strong> boards · <strong id="recent-jobs-count">{}</strong> recent jobs</p>"#,
        boards.len(),
        jobs.len()
    ));
```

3. Replace the entire "Recent jobs" table block (the `<h2>Recent jobs</h2>` push, the `<table>...<tbody>` push, the `if jobs.is_empty() { ... } else { ... }` loop, and the `</tbody></table>` push) with:

```rust
    // Recent jobs
    body.push_str(r#"<h2>Recent jobs</h2>"#);
    body.push_str(
        r#"<table class="rows"><thead><tr><th>id</th><th>state</th><th>priority</th><th>submitter</th><th>submitted</th></tr></thead><tbody id="recent-jobs-body">"#,
    );
    body.push_str(&recent_jobs_tbody(&jobs, now_ms));
    body.push_str("</tbody></table>");
```

4. Just before the final `super::html_shell(NavTab::Dashboard, "dashboard", body)`, append the client script tag:

```rust
    body.push_str(&format!(
        r#"<script src="/static/dashboard-live.js?v={PKG_VERSION}"></script>"#
    ));
```

(The `/static/dashboard-live.js` route is added in Task 5; until then the tag 404s harmlessly and the page still renders.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p paavo-web --lib recent_jobs_tbody`
Expected: PASS (2 tests).

- [ ] **Step 6: Format, lint, and commit**

```bash
cargo fmt -p paavo-web
cargo clippy -p paavo-web --all-targets -- -D warnings
git add crates/paavo-web/src/pages/dashboard.rs
git commit -m "paavo-web: extract recent_jobs_tbody + add live-region ids"
```

---

## Task 2: `feed.rs` core — `JobFeed`, `render_payload`, `spawn_poller`

**Files:**
- Create: `crates/paavo-web/src/feed.rs`
- Modify: `crates/paavo-web/src/lib.rs`

Pure logic + the background poller. No HTTP handler and no `AppState` change yet, so `feed.rs` compiles standalone.

- [ ] **Step 1: Create `feed.rs` with the core types**

Create `crates/paavo-web/src/feed.rs`:

```rust
//! Dashboard live feed: one background poller renders the "Recent jobs"
//! table from the read-only DB and fans it out to connected browsers
//! over SSE. See the design at
//! `docs/superpowers/specs/2026-06-16-paavo-web-live-dashboard-design.md`.

use crate::db::WebDb;
use crate::pages::dashboard::{recent_jobs_tbody, RECENT_JOBS_LIMIT};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// Fallback SSE payload, used only if the startup DB read fails; the
/// first successful poll replaces it. An empty "Recent jobs" table.
pub const EMPTY_PAYLOAD: &str =
    r#"{"count":0,"tbody":"<tr><td class=\"empty\" colspan=\"5\">no jobs yet</td></tr>"}"#;

/// Process-local latest-snapshot channel for the dashboard "Recent
/// jobs" table. Cloneable (AppState requirement); the single underlying
/// value (the latest SSE payload) is shared by the poller and every SSE
/// connection.
#[derive(Clone)]
pub struct JobFeed(Arc<watch::Sender<String>>);

impl JobFeed {
    /// Seed the channel with an initial payload.
    pub fn new(initial: String) -> Self {
        Self(Arc::new(watch::channel(initial).0))
    }

    /// A fresh receiver positioned at the current value.
    pub fn subscribe(&self) -> watch::Receiver<String> {
        self.0.subscribe()
    }

    /// Update the latest snapshot iff it changed. `send_if_modified`
    /// updates the stored value even with zero receivers (plain `send`
    /// is a no-op with no receivers and would leave the stored snapshot
    /// stale for the next connector).
    pub fn publish_if_changed(&self, payload: String) {
        self.0.send_if_modified(|cur| {
            if *cur != payload {
                *cur = payload;
                true
            } else {
                false
            }
        });
    }
}

/// Render the current SSE payload from the RO DB: `{count, tbody}` JSON.
/// `pub` so `main` can compute the startup seed and tests can assert it.
pub fn render_payload(db: &WebDb) -> paavo_db::Result<String> {
    let jobs = db.recent_jobs(RECENT_JOBS_LIMIT)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let tbody = recent_jobs_tbody(&jobs, now_ms);
    Ok(serde_json::json!({ "count": jobs.len(), "tbody": tbody }).to_string())
}

/// Spawn the single dashboard poller. `interval` is a parameter so
/// integration tests can run it at ~30 ms instead of the 1 s default.
/// A transient read error keeps the last snapshot and skips the tick.
pub fn spawn_poller(db: WebDb, feed: JobFeed, interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            match render_payload(&db) {
                Ok(payload) => feed.publish_if_changed(payload),
                Err(e) => tracing::warn!(
                    error = %e,
                    "dashboard feed poll failed; keeping last snapshot"
                ),
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

    #[test]
    fn render_payload_empty_db_reports_zero_count() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paavo.sqlite");
        let _ = Db::open(&path).unwrap();
        let webdb = WebDb::open(&path).unwrap();
        let p = render_payload(&webdb).unwrap();
        assert!(p.contains(r#""count":0"#), "got: {p}");
        assert!(p.contains("no jobs yet"), "got: {p}");
    }

    #[test]
    fn render_payload_reflects_inserted_job() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paavo.sqlite");
        let id = JobId::new();
        {
            let db = Db::open(&path).unwrap();
            paavo_db::JobRow::insert(db.raw_conn(), &sample_new_job(id), 0).unwrap();
        }
        let webdb = WebDb::open(&path).unwrap();
        let p = render_payload(&webdb).unwrap();
        assert!(p.contains(r#""count":1"#), "got: {p}");
        assert!(p.contains(&id.to_string()), "payload missing job id; got: {p}");
    }

    #[test]
    fn publish_if_changed_only_fires_on_change() {
        let feed = JobFeed::new("a".to_string());
        let mut rx = feed.subscribe();
        // Same value: no change.
        feed.publish_if_changed("a".to_string());
        assert!(!rx.has_changed().unwrap(), "unchanged publish should not notify");
        // New value: change.
        feed.publish_if_changed("b".to_string());
        assert!(rx.has_changed().unwrap(), "changed publish should notify");
        assert_eq!(*rx.borrow_and_update(), "b");
    }

    #[tokio::test]
    async fn spawn_poller_pushes_after_insert() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paavo.sqlite");
        let rw = Db::open(&path).unwrap(); // keep the writer alive for WAL visibility
        let webdb = WebDb::open(&path).unwrap();
        let initial = render_payload(&webdb).unwrap();
        let feed = JobFeed::new(initial);
        spawn_poller(webdb.clone(), feed.clone(), Duration::from_millis(20));
        let mut rx = feed.subscribe();

        let id = JobId::new();
        paavo_db::JobRow::insert(rw.raw_conn(), &sample_new_job(id), 0).unwrap();

        // Loop until the pushed snapshot contains the new job id.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!remaining.is_zero(), "poller never pushed the new job");
            if tokio::time::timeout(remaining, rx.changed()).await.is_err() {
                panic!("poller never pushed the new job (timeout)");
            }
            if rx.borrow_and_update().contains(&id.to_string()) {
                break;
            }
        }
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/paavo-web/src/lib.rs`, add `pub mod feed;` (keep the existing alphabetical-ish ordering — place it after `pub mod db;`):

```rust
pub mod app;
pub mod config;
pub mod db;
pub mod feed;
pub mod pages;
pub mod proxy;
pub mod time;
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p paavo-web --lib feed::`
Expected: PASS (4 tests: `render_payload_empty_db_reports_zero_count`, `render_payload_reflects_inserted_job`, `publish_if_changed_only_fires_on_change`, `spawn_poller_pushes_after_insert`).

- [ ] **Step 4: Format, lint, and commit**

```bash
cargo fmt -p paavo-web
cargo clippy -p paavo-web --all-targets -- -D warnings
git add crates/paavo-web/src/feed.rs crates/paavo-web/src/lib.rs
git commit -m "paavo-web: add JobFeed + render_payload + dashboard poller"
```

---

## Task 3: Wire `feed` into `AppState` and `main`

**Files:**
- Modify: `crates/paavo-web/src/proxy.rs`
- Modify: `crates/paavo-web/src/app.rs`
- Modify: `crates/paavo-web/src/main.rs`
- Modify: `crates/paavo-web/tests/smoke.rs`
- Modify: `crates/paavo-web/tests/proxy.rs`

Pure plumbing: add the field, the `FromRef` glue, fix every `AppState { .. }` construction site, and spawn the poller in `main`. No new endpoint yet. Verified by the existing suite staying green (adding a struct field breaks all constructors, so this task is "make it compile + pass again").

- [ ] **Step 1: Add the field to `AppState`**

In `crates/paavo-web/src/proxy.rs`, add the field to the `AppState` struct:

```rust
#[derive(Clone)]
pub struct AppState {
    /// Read-only sqlite handle.
    pub db: crate::db::WebDb,
    /// paavod HTTP client (for the SSE proxy; pages don't use it).
    pub paavod: PaavodClient,
    /// Dashboard live-feed channel (poller → SSE fan-out).
    pub feed: crate::feed::JobFeed,
}
```

- [ ] **Step 2: Add the `FromRef` glue**

In `crates/paavo-web/src/app.rs`, alongside the existing `FromRef` impls (after the `PaavodClient` one), add:

```rust
impl FromRef<AppState> for crate::feed::JobFeed {
    fn from_ref(s: &AppState) -> Self {
        s.feed.clone()
    }
}
```

- [ ] **Step 3: Build the feed + spawn the poller in `main`**

In `crates/paavo-web/src/main.rs`, add `use std::time::Duration;` at the top, and replace the state-construction block. Current:

```rust
    let db = paavo_web::db::WebDb::open(&sqlite_path)?;
    // paavod_url is parsed at startup so a malformed value fails
    // here, not on the first SSE proxy request.
    let paavod = paavo_web::proxy::PaavodClient::new(&cfg.web.paavod_url)?;
    let state = paavo_web::proxy::AppState { db, paavod };
```

Replace with:

```rust
    let db = paavo_web::db::WebDb::open(&sqlite_path)?;
    // paavod_url is parsed at startup so a malformed value fails
    // here, not on the first SSE proxy request.
    let paavod = paavo_web::proxy::PaavodClient::new(&cfg.web.paavod_url)?;
    // Dashboard live feed: seed the channel with the current table so
    // the first browser to connect gets real data, then poll for
    // changes every second and push them to connected dashboards.
    const DASHBOARD_POLL_INTERVAL: Duration = Duration::from_secs(1);
    let initial = paavo_web::feed::render_payload(&db)
        .unwrap_or_else(|_| paavo_web::feed::EMPTY_PAYLOAD.to_string());
    let feed = paavo_web::feed::JobFeed::new(initial);
    paavo_web::feed::spawn_poller(db.clone(), feed.clone(), DASHBOARD_POLL_INTERVAL);
    let state = paavo_web::proxy::AppState { db, paavod, feed };
```

- [ ] **Step 4: Fix the three test `AppState` constructors**

In `crates/paavo-web/tests/smoke.rs`, both constructors (in `fresh_app` around line 23, and in `job_detail_emits_data_since_seq_when_frames_exist` around line 362) — add the `feed` field:

```rust
    let state = AppState {
        db,
        paavod,
        feed: paavo_web::feed::JobFeed::new(paavo_web::feed::EMPTY_PAYLOAD.to_string()),
    };
```

(For the second site the field name is `db: webdb` — preserve it: `AppState { db: webdb, paavod, feed: paavo_web::feed::JobFeed::new(paavo_web::feed::EMPTY_PAYLOAD.to_string()) }`.)

In `crates/paavo-web/tests/proxy.rs`, in `paavo_web_router` (around line 60):

```rust
    let state = AppState {
        db,
        paavod,
        feed: paavo_web::feed::JobFeed::new(paavo_web::feed::EMPTY_PAYLOAD.to_string()),
    };
```

- [ ] **Step 5: Build and run the full crate suite to verify green**

Run: `cargo test -p paavo-web`
Expected: PASS — all pre-existing tests compile and pass unchanged (no behavior change; the feed is wired but no route consumes it yet).

- [ ] **Step 6: Format, lint, and commit**

```bash
cargo fmt -p paavo-web
cargo clippy -p paavo-web --all-targets -- -D warnings
git add crates/paavo-web/src/proxy.rs crates/paavo-web/src/app.rs crates/paavo-web/src/main.rs crates/paavo-web/tests/smoke.rs crates/paavo-web/tests/proxy.rs
git commit -m "paavo-web: thread JobFeed through AppState; spawn poller in main"
```

---

## Task 4: SSE endpoint `GET /api/dashboard/feed`

**Files:**
- Modify: `crates/paavo-web/src/feed.rs`
- Modify: `crates/paavo-web/src/app.rs`
- Create: `crates/paavo-web/tests/feed.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `crates/paavo-web/tests/feed.rs`:

```rust
//! Integration tests for the dashboard live feed (SSE).
//!
//! The dashboard feed is an SSE stream that never closes on its own, so
//! these tests drive the response body stream incrementally with a
//! timeout (unlike the per-job proxy tests, which use `to_bytes` because
//! that stream terminates on a `terminal` event).

use axum::body::Body;
use axum::http::Request;
use paavo_db::{Db, NewJob};
use paavo_proto::{BoardSelector, JobId, JobSource, Priority};
use paavo_web::db::WebDb;
use paavo_web::proxy::{AppState, PaavodClient};
use std::time::Duration;
use tempfile::tempdir;
use tower::ServiceExt;

/// Build a feed-enabled paavo-web router over a fresh temp DB. Returns
/// the live RW `Db` writer (keep it alive — it provides WAL visibility
/// for the RO reader and lets a test insert mid-stream), the TempDir
/// guard, and the Router. Spawns the poller at `interval`.
fn feed_app(interval: Duration) -> (tempfile::TempDir, Db, axum::Router) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let rw = Db::open(&path).unwrap();
    let webdb = WebDb::open(&path).unwrap();
    let initial = paavo_web::feed::render_payload(&webdb)
        .unwrap_or_else(|_| paavo_web::feed::EMPTY_PAYLOAD.to_string());
    let feed = paavo_web::feed::JobFeed::new(initial);
    paavo_web::feed::spawn_poller(webdb.clone(), feed.clone(), interval);
    let paavod = PaavodClient::new("http://127.0.0.1:1").expect("valid URL");
    let state = AppState {
        db: webdb,
        paavod,
        feed,
    };
    let app = paavo_web::app::build_router(state);
    (dir, rw, app)
}

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

/// Read SSE bytes from an open body stream until `needle` appears or
/// `timeout` elapses; returns the accumulated text.
async fn read_until<S>(stream: &mut S, needle: &str, timeout: Duration) -> String
where
    S: futures::Stream<Item = Result<bytes::Bytes, axum::Error>> + Unpin,
{
    use futures::StreamExt;
    let mut acc = String::new();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                acc.push_str(&String::from_utf8_lossy(&chunk));
                if acc.contains(needle) {
                    break;
                }
            }
            // stream error, clean EOF, or timeout: stop reading.
            Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
        }
    }
    acc
}

#[tokio::test]
async fn feed_emits_initial_snapshot_event() {
    // A long interval keeps the poller effectively idle; the immediate
    // snapshot comes from the seed, so this pins the on-connect push.
    let (_dir, _rw, app) = feed_app(Duration::from_secs(60));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/dashboard/feed")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(ct.starts_with("text/event-stream"), "wrong content-type: {ct}");
    let mut stream = resp.into_body().into_data_stream();
    let acc = read_until(&mut stream, "no jobs yet", Duration::from_secs(5)).await;
    assert!(acc.contains("event: recent-jobs"), "missing event name; got:\n{acc}");
    assert!(acc.contains("no jobs yet"), "missing empty-state snapshot; got:\n{acc}");
}

#[tokio::test]
async fn feed_pushes_update_when_job_inserted() {
    let (_dir, rw, app) = feed_app(Duration::from_millis(30));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/dashboard/feed")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let mut stream = resp.into_body().into_data_stream();

    // 1. Drain the initial empty snapshot.
    let snap = read_until(&mut stream, "no jobs yet", Duration::from_secs(5)).await;
    assert!(
        snap.contains("event: recent-jobs"),
        "initial snapshot missing; got:\n{snap}"
    );

    // 2. Insert a job via the live RW writer; the poller should push it.
    let id = JobId::new();
    paavo_db::JobRow::insert(rw.raw_conn(), &sample_new_job(id), 0).unwrap();

    // 3. Read until the new job id shows up in a pushed event.
    let upd = read_until(&mut stream, &id.to_string(), Duration::from_secs(5)).await;
    assert!(
        upd.contains(&id.to_string()),
        "feed did not push the new job; got:\n{upd}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p paavo-web --test feed`
Expected: FAIL — both tests get `404` (route not mounted) so the `assert_eq!(resp.status(), 200)` fails. (If compilation fails first because `dashboard_feed` is unmounted, that is the expected red state — proceed to implement.)

- [ ] **Step 3: Add the `dashboard_feed` handler**

Append to `crates/paavo-web/src/feed.rs` (after `spawn_poller`, before the `#[cfg(test)]` module). Add the imports at the top of the file too.

Top-of-file imports to add:

```rust
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use std::convert::Infallible;
```

Handler:

```rust
/// `GET /api/dashboard/feed` — emit the current "Recent jobs" snapshot
/// immediately, then one `recent-jobs` SSE event per change. The
/// immediate snapshot closes the SSR→connect gap and re-syncs every
/// `EventSource` auto-reconnect, so no `Last-Event-ID` handling is
/// needed. 15 s keep-alive comments match the per-job proxy.
pub async fn dashboard_feed(State(feed): State<JobFeed>) -> impl IntoResponse {
    let mut rx = feed.subscribe();
    let stream = async_stream::stream! {
        // borrow_and_update marks the current value seen so the next
        // changed() waits for the *next* change. The Ref is dropped at
        // the end of each statement — never held across an .await.
        let initial = rx.borrow_and_update().clone();
        yield Ok::<Event, Infallible>(Event::default().event("recent-jobs").data(initial));
        while rx.changed().await.is_ok() {
            let payload = rx.borrow_and_update().clone();
            yield Ok(Event::default().event("recent-jobs").data(payload));
        }
    };
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
```

- [ ] **Step 4: Mount the route**

In `crates/paavo-web/src/app.rs`, inside `build_router`, add the route next to the existing SSE proxy route:

```rust
        .route("/api/jobs/:id/stream", get(crate::proxy::stream_job))
        .route("/api/dashboard/feed", get(crate::feed::dashboard_feed))
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p paavo-web --test feed`
Expected: PASS (2 tests).

- [ ] **Step 6: Format, lint, and commit**

```bash
cargo fmt -p paavo-web
cargo clippy -p paavo-web --all-targets -- -D warnings
git add crates/paavo-web/src/feed.rs crates/paavo-web/src/app.rs crates/paavo-web/tests/feed.rs
git commit -m "paavo-web: add /api/dashboard/feed SSE endpoint"
```

---

## Task 5: Client asset `dashboard-live.js` + static route

**Files:**
- Create: `crates/paavo-web/src/assets/dashboard-live.js`
- Modify: `crates/paavo-web/src/app.rs`
- Modify: `crates/paavo-web/tests/smoke.rs`

- [ ] **Step 1: Write the failing smoke tests**

Append to `crates/paavo-web/tests/smoke.rs`:

```rust
#[tokio::test]
async fn static_dashboard_live_js_serves_with_correct_headers() {
    let (_d, app) = fresh_app();
    let (status, body) = fetch(app, "/static/dashboard-live.js").await;
    assert_eq!(status, 200);
    assert!(
        body.contains("EventSource") || body.contains("recent-jobs"),
        "dashboard-live.js content marker missing; got first 200 chars: {}",
        &body.chars().take(200).collect::<String>()
    );
}

#[tokio::test]
async fn dashboard_wires_live_feed_consumer() {
    // The dashboard must carry the live-region ids the JS targets and
    // load the consumer script, or the table is silently inert.
    let (_d, app) = fresh_app();
    let (status, body) = fetch(app, "/").await;
    assert_eq!(status, 200);
    assert!(
        body.contains(r#"id="recent-jobs-body""#),
        "dashboard missing #recent-jobs-body; body: {body}"
    );
    assert!(
        body.contains(r#"id="recent-jobs-count""#),
        "dashboard missing #recent-jobs-count; body: {body}"
    );
    assert!(
        body.contains(r#"<script src="/static/dashboard-live.js?v="#),
        "dashboard missing dashboard-live.js script tag; body: {body}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p paavo-web --test smoke dashboard_live`
Expected: FAIL — `static_dashboard_live_js_serves_with_correct_headers` gets a 404 (route missing). `dashboard_wires_live_feed_consumer` may already pass for the ids/script (added in Task 1) but is grouped here for the wiring contract; the route test is the red one.

- [ ] **Step 3: Create the client JS asset**

Create `crates/paavo-web/src/assets/dashboard-live.js`:

```javascript
// paavo-web — live "Recent jobs" updater for the dashboard (/).
//
// Loaded by the dashboard via `<script src="/static/dashboard-live.js?v=...">`.
// Opens an EventSource against `/api/dashboard/feed` and, on each
// `recent-jobs` event, swaps the table body and updates the row count.
// The payload is a JSON envelope `{count, tbody}` whose `tbody` is
// server-rendered, already-escaped HTML (see crates/paavo-web/src/feed.rs
// and pages/dashboard.rs::recent_jobs_tbody) — so assigning it via
// innerHTML introduces no new XSS surface.
//
// Vanilla DOM, no framework. No-op on any page lacking #recent-jobs-body.
(function () {
  'use strict';

  var body = document.getElementById('recent-jobs-body');
  if (!body) return; // not the dashboard
  var count = document.getElementById('recent-jobs-count');

  var es = new EventSource('/api/dashboard/feed');

  es.addEventListener('recent-jobs', function (e) {
    var d;
    try {
      d = JSON.parse(e.data);
    } catch (_err) {
      return; // ignore a malformed frame; the next push corrects it
    }
    if (typeof d.tbody === 'string') body.innerHTML = d.tbody;
    if (count && typeof d.count === 'number') count.textContent = d.count;
  });

  // EventSource auto-reconnects with backoff; the server re-sends the
  // current snapshot on connect, so there is nothing to recover here.
})();
```

- [ ] **Step 4: Add the serve handler + route**

In `crates/paavo-web/src/app.rs`, add a serve function next to `serve_live_log_js`:

```rust
/// `/static/dashboard-live.js` — serves the dashboard live-feed
/// consumer. Same caching contract as `/static/live-log.js`: baked in
/// at compile time, day-long cache, must-revalidate, version-busted via
/// `?v={CARGO_PKG_VERSION}` on the `<script src=...>` link.
async fn serve_dashboard_live_js() -> impl IntoResponse {
    const JS: &str = include_str!("assets/dashboard-live.js");
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/javascript; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400, must-revalidate"),
            ),
        ],
        JS,
    )
}
```

And mount it next to the other static asset routes in `build_router`:

```rust
        .route("/static/live-log.js", get(serve_live_log_js))
        .route("/static/dashboard-live.js", get(serve_dashboard_live_js))
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p paavo-web --test smoke dashboard`
Expected: PASS (`static_dashboard_live_js_serves_with_correct_headers`, `dashboard_wires_live_feed_consumer`).

- [ ] **Step 6: Run the full crate suite, format, lint, and commit**

```bash
cargo test -p paavo-web
cargo fmt -p paavo-web
cargo clippy -p paavo-web --all-targets -- -D warnings
git add crates/paavo-web/src/assets/dashboard-live.js crates/paavo-web/src/app.rs crates/paavo-web/tests/smoke.rs
git commit -m "paavo-web: serve dashboard-live.js + wire the dashboard consumer"
```

---

## Task 6: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Full crate test + lint sweep**

Run:

```
cargo test -p paavo-web
cargo fmt -p paavo-web -- --check
cargo clippy -p paavo-web --all-targets -- -D warnings
```

Expected: all green; `fmt --check` reports no diffs; clippy clean.

- [ ] **Step 2: Workspace build (no cross-crate breakage)**

Run: `cargo build --workspace`
Expected: success (paavo-web is the only crate changed; this confirms the `AppState` field addition did not break any other consumer — there are none, but verify).

- [ ] **Step 3: Manual smoke (documented; run if a dev environment is available)**

Per the spec §8: start paavo-web against a dev config, open `/`, then submit a job from another terminal (`paavo-cli run ...`, or with `PAAVO_FAKE_RUNNER=1` per `manual-smoke.nu`). Without touching the page, confirm the new job appears in "Recent jobs" within ~1 s and its state cell advances `Submitted → Building → Running → <terminal>` in place, with the "N recent jobs" count staying consistent.

---

## Self-Review notes (for the implementer)

- **Spec coverage:** §3.1 poll-in-web + SSE (Tasks 2–4), §3.2 `watch` channel (Task 2 `JobFeed`), §3.3 whole-`<tbody>` swap (Task 1 renderer + Task 5 JS), §3.4 `{count, tbody}` envelope (Task 2 `render_payload` + Task 5 JS), §4.5 immediate snapshot + 15 s keep-alive (Task 4), §5 failure modes (poll-error skip in Task 2 `spawn_poller`; snapshot-on-connect in Task 4; `send_if_modified` zero-receiver correctness in Task 2), §7 testing (unit Tasks 1–2, integration Task 4, smoke Task 5), §9 out-of-scope respected (board fleet + `/jobs` untouched).
- **Type consistency:** `JobFeed`, `render_payload`, `EMPTY_PAYLOAD`, `spawn_poller`, `dashboard_feed` names are identical across feed.rs, app.rs, main.rs, and tests. The SSE event name is `recent-jobs` in both the handler (Task 4) and the JS listener (Task 5). `RECENT_JOBS_LIMIT` is the single jobs cap used by `render` (Task 1) and `render_payload` (Task 2).
- **Known platform note:** the integration tests rely on a RO SQLite reader seeing a concurrently-open RW writer's commits via WAL — the production paavod-writer / paavo-web-reader topology. `feed_app` keeps the RW `Db` alive for exactly this reason.
