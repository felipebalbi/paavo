//! Typed `fetch` wrappers over the paavo-web JSON API (`/api/...`).
//!
//! Every endpoint paavo-web exposes (see its `src/api/` handlers + the design
//! spec §5) gets one async function here that issues the request with
//! `gloo_net` and deserializes the JSON body into the corresponding
//! `paavo-proto` wire type. URLs are **same-origin relative**: the SPA is
//! served by the very backend it calls, so no base URL is configured.
//!
//! Errors collapse to `String` (via [`err`]) so a `LocalResource` can surface
//! them directly in an error branch — the UI has nothing actionable to do with
//! a structured transport error beyond showing it.

use paavo_proto::{
    BoardView, DashboardOverview, JobListItem, JobView, LogFrame, Page, ScheduleView,
};

/// Stringify any `Display` error (gloo-net transport error, serde decode
/// error) into a render-ready message.
fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Percent-encode one query-parameter value with the platform's
/// `encodeURIComponent`, so a query containing spaces, `&`, `#`, `+`, etc.
/// round-trips correctly to the server's `q`/`id` parsing.
fn encode(value: &str) -> String {
    js_sys::encode_uri_component(value).into()
}

/// `GET /api/jobs?q=&page=&per_page=&as_of=` — one page of the (optionally
/// fuzzy-filtered) jobs index with an explicit page size. Blank `q` returns the
/// time-ordered list; `as_of` pins the page to `submitted_at <= as_of` for
/// stable paging.
pub async fn jobs_page(
    q: &str,
    page: u32,
    per_page: u32,
    as_of: Option<i64>,
) -> Result<Page<JobListItem>, String> {
    let mut url = format!("/api/jobs?page={page}&per_page={per_page}&q={}", encode(q));
    if let Some(t) = as_of {
        url.push_str(&format!("&as_of={t}"));
    }
    fetch_json(&url).await
}

/// `GET /api/jobs/{id}` — one job's full view (404 → `Err`).
pub async fn job(id: &str) -> Result<JobView, String> {
    fetch_json(&format!("/api/jobs/{}", encode(id))).await
}

/// `GET /api/jobs/{id}/log?offset=&limit=1000` — a backfill page of persisted
/// log frames (oldest first). The live tail is a separate `EventSource`
/// (`/api/jobs/{id}/stream`), wired up by the job-detail task.
pub async fn job_log(id: &str, offset: u32) -> Result<Vec<LogFrame>, String> {
    fetch_json(&format!(
        "/api/jobs/{}/log?offset={offset}&limit=1000",
        encode(id)
    ))
    .await
}

/// `GET /api/boards?page=&per_page=&q=` — one page of the (optionally
/// filtered) board fleet with an explicit page size. The general form
/// backing [`boards`] (which fixes `per_page=20`). A non-blank `q` narrows
/// by an `id`/`kind` substring matched server-side across the *whole*
/// table.
pub async fn boards_page(page: u32, per_page: u32, q: &str) -> Result<Page<BoardView>, String> {
    fetch_json(&format!(
        "/api/boards?page={page}&per_page={per_page}&q={}",
        encode(q)
    ))
    .await
}

/// `GET /api/boards?page=&per_page=20&q=` — one page of the (optionally
/// filtered) board fleet at the default 20-row page size. Thin wrapper
/// over [`boards_page`].
pub async fn boards(page: u32, q: &str) -> Result<Page<BoardView>, String> {
    boards_page(page, 20, q).await
}

/// `GET /api/dashboard` — the consolidated landing-page payload: exact
/// aggregate counts plus the recent-jobs and fleet display slices, in one
/// bounded response. Replaces the dashboard's old wide jobs+boards fetch.
pub async fn dashboard() -> Result<DashboardOverview, String> {
    fetch_json("/api/dashboard").await
}

/// `GET /api/schedules?page=&per_page=20` — one page of cron schedules.
pub async fn schedules(page: u32) -> Result<Page<ScheduleView>, String> {
    fetch_json(&format!("/api/schedules?page={page}&per_page=20")).await
}

/// Issue a GET for `url` and decode the JSON body as `T`. Shared by every
/// wrapper above so the request/decode/error plumbing lives in one place.
async fn fetch_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, String> {
    gloo_net::http::Request::get(url)
        .send()
        .await
        .map_err(err)?
        .json()
        .await
        .map_err(err)
}
