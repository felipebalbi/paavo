# Design: live dashboard — push new/changed jobs to `/` over SSE

**Status**: design approved 2026-06-16. paavo-web-only change. No
paavod change, no schema migration, no wire (`WireMessage` / `LogFrame`)
shape change.

---

## 1. Goal

The paavo-web dashboard (`/`) renders the "Recent jobs" table by reading
`paavo.sqlite` once at request time, so a newly-submitted job only
appears after a manual page refresh. This design makes the "Recent jobs"
table **update on its own**: a job submitted from anywhere shows up
within ~1 s, and its state transitions (`Submitted → Building → Running
→ Passed/Failed/...`) update in place — no refresh, no user action.

The mechanism is a server push: paavo-web runs one background poller over
its read-only SQLite handle and fans the current table out to every
connected browser over Server-Sent Events (SSE), reusing the same
`EventSource` + baked-asset pattern the `/jobs/:id` live-log already
uses.

## 2. Background: the precise gap

paavo-web is a **read-only viewer**. It opens `paavo.sqlite` in WAL + RO
mode (`db::WebDb::open`) and every page handler reads at request time:

- `pages/dashboard.rs::render` reads `db.all_boards()` + `db.recent_jobs(20)`
  and renders two static HTML tables ("Board fleet", "Recent jobs").
- The page has no live channel. The only existing push surface is
  `GET /api/jobs/:id/stream` (`proxy.rs`), an SSE proxy that bridges
  **one job's** NDJSON log stream from paavod into a browser
  `EventSource` (`assets/live-log.js`). There is **no dashboard-level
  "a job changed" feed** anywhere.

Jobs enter the system by two paths, both of which write the DB:

| Path                             | Writer                                            |
|----------------------------------|---------------------------------------------------|
| `POST /jobs` (paavo-cli, ad-hoc) | `paavod::routes::jobs::post_jobs` → `enqueue_job` |
| cron scheduler (nightly soaks)   | `paavod::cron` → `enqueue_job` directly (no HTTP) |

Because both terminate in a SQLite write, **polling the DB is the one
observation point that sees every job from every source** — including
scheduler enqueues that never touch paavod's HTTP surface. State
transitions (`Building`, `Running`, terminal) are likewise DB writes the
poller observes for free.

## 3. Decisions

Four decisions were made during design review; rejected alternatives are
recorded so the rationale survives.

### 3.1 Push origin: poll-in-web + SSE fan-out (not a paavod event broker)

paavo-web spawns **one** background task that polls its own RO SQLite on
an interval, renders the "Recent jobs" fragment, and publishes it to a
process-local channel that all connected `EventSource`s drain.

*Why:* keeps paavo-web's "read-only viewer" identity intact — zero
paavod changes, no new cross-crate wire format, no in-memory global
broker to instrument at every enqueue/transition site. Polling the DB
(the source of truth) also captures scheduler-enqueued jobs and all
state transitions with no extra producer-side code. SQLite WAL + RO reads
of ≤20 rows are sub-millisecond; one shared poller serves N viewers with
one query per tick.

*Rejected:* "paavod grows a global lifecycle broadcast channel; paavo-web
proxies it like the per-job log stream." Architecturally consistent with
the existing proxy and gives sub-second push, but it touches two crates,
adds a global in-memory broker whose every publish site (HTTP submit +
cron + each dispatch transition) must be wired and kept wired, and still
needs a DB read to render row data. More moving parts for a latency win
("watch my job appear and start") that ~1 s polling already satisfies.

*Rejected:* "client-side `setInterval` re-fetch of a fragment." Simplest,
but it is a client poll — not the server push requested — and re-fetches
on every tick regardless of change.

### 3.2 Transport channel: `tokio::sync::watch` (not `broadcast`)

The poller publishes the rendered fragment through a
`watch::Sender<String>`; each SSE connection drains a `watch::Receiver`.

*Why:* `watch` keeps only the **latest** value and hands it to every
subscriber, including ones that join later. That is exactly the semantic
this feature needs — each push is the *entire current* "Recent jobs"
table, so intermediate states are irrelevant and a late or reconnecting
subscriber simply wants the newest snapshot. `watch` gives every new
connection an immediate current snapshot with no lag/backpressure
bookkeeping.

*Rejected:* `broadcast` (capacity 256, per-subscriber lag handling — the
per-job log broker's choice). Correct but unnecessary: log frames are an
ordered append stream where every element matters and a slow consumer
must learn it lagged; the dashboard table is a single
latest-value-wins cell where older snapshots are pure waste.

### 3.3 DOM update: server renders `<tbody>`, browser swaps it

On each push the browser replaces the "Recent jobs" `<tbody>` innerHTML
with server-rendered HTML and updates the row-count number. Row HTML is
produced by **one** Rust function shared with the SSR page render.

*Why:* a single source of truth for row markup — state CSS classes
(`state_class`), relative timestamps (`relative_to_now`), and HTML
escaping (`html_escape`) live in Rust only and can never drift between
the first paint and a live update. Swapping the whole `<tbody>` (≤20
small rows, a few KB, only when something changed) also eliminates an
entire class of client-side merge/order/dedup bugs.

*Rejected:* "push per-row `<tr>` + id; JS finds-by-id, prepends, re-sorts,
trims to 20." Enables a per-row flash animation but reintroduces
ordering/trimming/dedup logic in JS. *Rejected:* "push JSON, render rows
in JS" — smallest payload, but duplicates row formatting in JavaScript,
the exact drift `live-log.js` already had to guard against.

### 3.4 Payload: a `{count, tbody}` JSON envelope

The SSE `data` is `serde_json::to_string` of `{"count": <usize>, "tbody":
"<rendered rows html>"}`.

*Why:* the row HTML stays 100% server-rendered (per §3.3); the envelope
just carries it alongside the scalar count so the "*N* recent jobs"
header stays consistent with the table after a swap. serde_json escapes
the embedded newlines/quotes, so the envelope is a single line — safe to
hand to an SSE `data:` field verbatim.

*Rejected:* "push bare `<tbody>` HTML; derive the count in JS by counting
rows." Avoids the envelope but needs per-row markers and must filter the
`no jobs yet` empty-state row — more JS for no real gain.

## 4. Architecture

All changes live in `crates/paavo-web`.

### 4.1 Data flow

```
            paavo-web process
 ┌───────────────────────────────────────────────┐
 │  poller task  (1 per process, spawned in main) │
 │    every DASHBOARD_POLL_INTERVAL:              │
 │      db.recent_jobs(LIMIT) → render fragment   │
 │      → {count,tbody} JSON → publish_if_changed │
 │              │                                 │
 │              ▼   Arc<watch::Sender<String>>    │
 │        JobFeed (on AppState)                   │
 │              │ subscribe()                     │
 │      ┌───────┴────────┐                        │
 │      ▼                ▼                        │
 │  /api/dashboard/feed  /api/dashboard/feed      │
 │   (SSE conn A)        (SSE conn B)             │
 └──────┼────────────────┼────────────────────────┘
        ▼                ▼
   EventSource      EventSource     ← dashboard-live.js
   swap <tbody>     swap <tbody>
```

### 4.2 `JobFeed` (new `src/feed.rs`)

```rust
/// Process-local latest-snapshot channel for the dashboard "Recent
/// jobs" table. Holds an Arc'd watch Sender so it is Clone (AppState
/// requirement) and a single underlying value (the latest SSE payload)
/// is shared by the poller and every SSE connection.
#[derive(Clone)]
pub struct JobFeed(std::sync::Arc<tokio::sync::watch::Sender<String>>);

impl JobFeed {
    /// Seed the channel with an initial payload (computed once at
    /// startup so the first subscriber gets real data, not an empty
    /// flash).
    pub fn new(initial: String) -> Self {
        Self(std::sync::Arc::new(tokio::sync::watch::channel(initial).0))
    }

    /// A fresh receiver positioned at the current value.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<String> {
        self.0.subscribe()
    }

    /// Update the latest snapshot iff it changed. Uses send_if_modified
    /// so the stored value is updated even when zero browsers are
    /// connected (plain `send` is a no-op with no receivers and would
    /// leave the stored snapshot stale for the next connector).
    pub fn publish_if_changed(&self, payload: String) {
        self.0.send_if_modified(|cur| {
            if *cur != payload { *cur = payload; true } else { false }
        });
    }
}
```

Change-gating lives here (`*cur != payload`) so the poller publishes only
real changes; idle ticks notify nobody.

### 4.3 Renderer (shared, in `pages/dashboard.rs`)

Extract the "Recent jobs" row rendering from `render` into:

```rust
/// Cap on rows shown in the "Recent jobs" table. Shared by the SSR
/// dashboard render and the live feed so their row sets + counts match.
pub(crate) const RECENT_JOBS_LIMIT: u32 = 20;

/// Render the inner HTML of the "Recent jobs" <tbody> (the <tr> rows, or
/// the single `no jobs yet` empty-state row). Single source of truth for
/// row markup, escaping, state classes, and relative timestamps.
pub(crate) fn recent_jobs_tbody(jobs: &[paavo_db::JobRow], now_ms: i64) -> String { ... }
```

- `render` calls `recent_jobs_tbody`, emitting it inside
  `<tbody id="recent-jobs-body">...</tbody>`, and tags the jobs count as
  `<strong id="recent-jobs-count">{n}</strong> recent jobs`. The board
  count `<strong>` is untouched (boards are not live). `render` replaces
  its literal `recent_jobs(20)` with `recent_jobs(RECENT_JOBS_LIMIT)`.
- `render` appends `<script src="/static/dashboard-live.js?v={PKG_VERSION}"></script>`
  to the body, after the tables, mirroring how `job_detail.rs` injects
  `live-log.js`.

### 4.4 Poller (in `src/feed.rs`)

```rust
/// Render the current SSE payload from the RO DB: {count, tbody} JSON.
/// `pub` so `main` can compute the startup seed and tests can assert it.
pub fn render_payload(db: &WebDb) -> paavo_db::Result<String> {
    let jobs = db.recent_jobs(pages::dashboard::RECENT_JOBS_LIMIT)?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let tbody = pages::dashboard::recent_jobs_tbody(&jobs, now_ms);
    Ok(serde_json::json!({ "count": jobs.len(), "tbody": tbody }).to_string())
}

/// Spawn the single dashboard poller. `interval` is a parameter so
/// integration tests can run it at ~50 ms instead of the 1 s default.
pub fn spawn_poller(db: WebDb, feed: JobFeed, interval: std::time::Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            match render_payload(&db) {
                Ok(payload) => feed.publish_if_changed(payload),
                Err(e) => tracing::warn!(error = %e,
                    "dashboard feed poll failed; keeping last snapshot"),
            }
        }
    });
}
```

A transient read error keeps the last good snapshot and skips the tick —
the table never blanks on a momentary WAL hiccup. `RECENT_JOBS_LIMIT` is
shared so the poller's row set and count are byte-identical to what the
SSR page would render.

### 4.5 SSE endpoint (in `src/feed.rs`)

```rust
/// GET /api/dashboard/feed — emit the current "Recent jobs" snapshot
/// immediately, then one `recent-jobs` event per change. Same 15 s
/// keep-alive as the per-job proxy.
pub async fn dashboard_feed(State(feed): State<JobFeed>) -> impl IntoResponse {
    let mut rx = feed.subscribe();
    let stream = async_stream::stream! {
        // Immediate snapshot: closes the SSR→connect gap and re-syncs
        // every auto-reconnect, so no Last-Event-ID handling is needed.
        let initial = rx.borrow_and_update().clone();
        yield Ok::<_, std::convert::Infallible>(
            Event::default().event("recent-jobs").data(initial));
        while rx.changed().await.is_ok() {
            let payload = rx.borrow_and_update().clone();
            yield Ok(Event::default().event("recent-jobs").data(payload));
        }
    };
    Sse::new(stream).keep_alive(
        KeepAlive::new().interval(Duration::from_secs(15)).text("keep-alive"))
}
```

`rx.changed()` returns `Err` only when the sender is dropped (process
shutdown); the loop ends and the stream closes cleanly.

### 4.6 Wiring

- `proxy.rs`: add `feed: JobFeed` to `AppState`.
- `app.rs`: add `impl FromRef<AppState> for JobFeed` (mirrors the
  existing `WebDb` / `PaavodClient` impls); add routes
  `.route("/api/dashboard/feed", get(crate::feed::dashboard_feed))` and
  `.route("/static/dashboard-live.js", get(serve_dashboard_live_js))`;
  add `serve_dashboard_live_js` (an `include_str!` of
  `assets/dashboard-live.js` with the same content-type + cache headers
  as `serve_live_log_js`).
- `lib.rs`: `pub mod feed;`.
- `main.rs`: build the seed payload best-effort, construct the feed,
  spawn the poller, put the feed on state:
  ```rust
  const DASHBOARD_POLL_INTERVAL: Duration = Duration::from_secs(1);
  let initial = paavo_web::feed::render_payload(&db)
      .unwrap_or_else(|_| paavo_web::feed::EMPTY_PAYLOAD.to_string());
  let feed = paavo_web::feed::JobFeed::new(initial);
  paavo_web::feed::spawn_poller(db.clone(), feed.clone(), DASHBOARD_POLL_INTERVAL);
  let state = paavo_web::proxy::AppState { db, paavod, feed };
  ```
  `EMPTY_PAYLOAD` is a `pub const &str` holding a
  `{count:0, tbody:"<no jobs yet row>"}` envelope, used only if the
  startup read fails; the first successful poll replaces it.

### 4.7 Client (`assets/dashboard-live.js`, new baked asset)

Vanilla DOM, no framework — same house style as `live-log.js`. No-op on
any page lacking `#recent-jobs-body`, so it is harmless if ever loaded
elsewhere.

```js
(function () {
  'use strict';
  const body = document.getElementById('recent-jobs-body');
  if (!body) return;                       // not the dashboard
  const count = document.getElementById('recent-jobs-count');
  const es = new EventSource('/api/dashboard/feed');
  es.addEventListener('recent-jobs', function (e) {
    let d; try { d = JSON.parse(e.data); } catch (_err) { return; }
    if (typeof d.tbody === 'string') body.innerHTML = d.tbody;  // server-rendered, pre-escaped
    if (count && typeof d.count === 'number') count.textContent = d.count;
  });
  // EventSource auto-reconnects with backoff; the server re-sends the
  // current snapshot on connect, so there is nothing to recover here.
})();
```

`body.innerHTML = d.tbody` is safe: the fragment is built in Rust with
the same `html_escape` the SSR page uses, so escaping already happened
server-side — no new XSS surface.

## 5. Failure modes & edge cases

- **SSR→connect race** (page rendered at T0, `EventSource` connects at
  T0+ε, a job arrives in the gap): the immediate-snapshot-on-connect
  re-syncs the client to the latest. Closed.
- **No jobs yet:** `recent_jobs_tbody` renders the existing
  `no jobs yet` empty-state row; the swap behaves identically.
- **Poller DB error:** log `warn`, keep the last published snapshot, skip
  the tick. The table never blanks on a transient read error.
- **paavo-web restart / network blip:** `EventSource` auto-reconnects;
  the fresh process re-seeds the channel and the reconnect gets an
  immediate snapshot.
- **No browsers connected:** `publish_if_changed` uses `send_if_modified`,
  which updates the stored snapshot even with zero receivers, so the next
  browser to connect gets the truly-current table, not a stale one.
- **Many viewers:** exactly one `recent_jobs` query per interval
  regardless of viewer count; each browser is a cheap `watch` subscriber.
- **Relative timestamps:** each push re-renders "*N* minutes ago" fresh,
  so a long-open dashboard is *less* stale than today. Absolute drift
  between pushes is unchanged and out of scope.
- **Latency:** ~1 s worst case (one poll interval). Acceptable for the
  "watch my job appear and start running" use case.

## 6. Progressive enhancement

The SSR page already renders a complete, correct table. `dashboard-live.js`
is pure enhancement: with JS disabled, or if `/api/dashboard/feed` fails,
the page behaves exactly as today (manual refresh). Nothing regresses.

## 7. Testing strategy

Mirrors the existing `tests/proxy.rs` + `tests/smoke.rs` harness (build
the router over a temp SQLite; seed rows via a paavo-db RW handle to the
same file — the RO `WebDb` sees them through WAL).

1. `pages/dashboard.rs` (`#[cfg(test)]`) — `recent_jobs_tbody` renders the
   `no jobs yet` row for an empty slice, and for a populated slice
   contains each job id, its `state_class`, and is byte-stable for
   identical input (the property that gates the poller's publish).
2. `feed.rs` (`#[cfg(test)]`) — `render_payload` over a temp db returns
   valid JSON with a numeric `count` and an HTML `tbody`, and the payload
   differs after a new job row is inserted (and after a row's state
   changes).
3. `tests/feed.rs` (new integration) — with `spawn_poller` at a ~50 ms
   test interval:
   - `GET /api/dashboard/feed` responds `200` with content-type
     `text/event-stream` and emits an immediate `recent-jobs` snapshot
     event.
   - After inserting a new job row, a live connection receives an updated
     `recent-jobs` event whose `tbody` contains the new job id within a
     bounded timeout.
4. `tests/smoke.rs` — the dashboard HTML contains `id="recent-jobs-body"`,
   `id="recent-jobs-count"`, and the `<script src="/static/dashboard-live.js…">`
   tag; `GET /static/dashboard-live.js` is served with a JavaScript
   content-type.

The `dashboard-live.js` DOM swap is covered by the SSR-attribute assertion
plus the §8 manual smoke; no JS test harness is stood up for this change.

## 8. Definition of done

- `cargo test -p paavo-web` green; workspace `fmt` + `clippy` clean.
- Manual smoke: open `/`, then submit a job from another terminal
  (`paavo-cli run ...` or `PAAVO_FAKE_RUNNER=1`). Without touching the
  page, the new job appears in "Recent jobs" within ~1 s and its state
  cell advances `Submitted → Building → Running → <terminal>` in place.
  The "*N* recent jobs" count stays consistent with the table.

## 9. Scope — explicitly out

- **Board fleet live updates.** The "Board fleet" table stays static
  (refresh to update); only "Recent jobs" is live.
- **The `/jobs` index page.** Same poller/feed could drive it later;
  not in this change.
- **Per-row flash/highlight animation.** Precluded by the whole-`<tbody>`
  swap (§3.3); revisit only if operators ask.
- **Configurable poll interval via `paavo.toml`.** Hardcoded 1 s const;
  promote to `[web]` config if a deployment ever needs it.
- **paavod-side push / global event broker** (§3.1 rejected).
- **Reconnect `Last-Event-ID` resync.** Unnecessary — snapshot-on-connect
  (§4.5) re-syncs every reconnect.
- **Absolute-timestamp drift on a long-open page.** Pre-existing; each
  push re-renders relative times, which only improves it.

## 10. Files touched

| File | Change |
| --- | --- |
| `crates/paavo-web/src/feed.rs` | **New.** `JobFeed`, `render_payload`, `EMPTY_PAYLOAD`, `spawn_poller`, `dashboard_feed` SSE handler + unit/integration-supporting tests |
| `crates/paavo-web/src/lib.rs` | `pub mod feed;` |
| `crates/paavo-web/src/pages/dashboard.rs` | Extract `recent_jobs_tbody` + `RECENT_JOBS_LIMIT`; add `id="recent-jobs-body"` / `id="recent-jobs-count"`; append `dashboard-live.js` script tag; unit tests |
| `crates/paavo-web/src/proxy.rs` | Add `feed: JobFeed` to `AppState` |
| `crates/paavo-web/src/app.rs` | `FromRef<AppState> for JobFeed`; routes `/api/dashboard/feed` + `/static/dashboard-live.js`; `serve_dashboard_live_js` |
| `crates/paavo-web/src/main.rs` | `DASHBOARD_POLL_INTERVAL`; build feed, spawn poller, put feed on state |
| `crates/paavo-web/src/assets/dashboard-live.js` | **New.** `EventSource` consumer that swaps `<tbody>` + count |
| `crates/paavo-web/tests/feed.rs` | **New.** SSE integration tests (snapshot-on-connect; update-on-insert) |
| `crates/paavo-web/tests/smoke.rs` | Assert dashboard ids + script tag; assert `/static/dashboard-live.js` served |

**paavod:** unchanged. **Migration:** none. **Proto change:** none.
**Estimated size:** ~250–350 LOC including tests.
