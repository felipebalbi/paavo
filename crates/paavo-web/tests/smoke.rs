//! Smoke tests for the SPA-era paavo-web router: the embedded-asset
//! root, the JSON read endpoints (over an empty DB), and the SSE log
//! proxy's pre-flight error branches. The richer index/search and live
//! SSE behaviours have their own integration files (`api_jobs.rs`,
//! `api_events.rs`); this file just pins the route table + envelope
//! shapes.

use axum::body::{to_bytes, Body};
use axum::http::Request;
use paavo_db::Db;
use paavo_proto::{BoardView, JobListItem, Page, ScheduleView};
use paavo_web::db::WebDb;
use paavo_web::index::LiveState;
use paavo_web::proxy::{AppState, PaavodClient};
use tempfile::tempdir;
use tower::ServiceExt;

/// Open a fresh empty DB and build a router around it. Keeps the
/// `TempDir` alive on the stack so the sqlite file isn't unlinked while
/// the test is still reading it. No poller is spawned — these tests
/// either hit the empty index or routes that don't read it.
///
/// The `PaavodClient` points at 127.0.0.1:1 (nothing listens there);
/// the SSE-proxy tests rely on the resulting clean connect-refused 502.
fn fresh_app() -> (tempfile::TempDir, axum::Router) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let _ = Db::open(&path).unwrap(); // run migrations
    let db = WebDb::open(&path).unwrap();
    let paavod = PaavodClient::new("http://127.0.0.1:1").expect("valid URL");
    let state = AppState {
        db,
        paavod,
        live: LiveState::new(),
    };
    let app = paavo_web::app::build_router(state);
    (dir, app)
}

/// Issue a GET and return `(status, content_type, body)`.
async fn fetch(app: axum::Router, uri: &str) -> (axum::http::StatusCode, String, String) {
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, ct, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn root_serves_embedded_html_shell() {
    // `/` falls through to `crate::embed::serve`, which returns either
    // the embedded `dist/index.html` or the not-built placeholder — in
    // both cases a non-empty `text/html` body. This is the SPA shell
    // the WASM app boots from.
    let (_d, app) = fresh_app();
    let (status, ct, body) = fetch(app, "/").await;
    assert_eq!(status, 200);
    assert!(ct.starts_with("text/html"), "wrong content-type: {ct}");
    assert!(!body.is_empty(), "empty shell body");
}

#[tokio::test]
async fn unknown_path_falls_back_to_spa_shell() {
    // A virtual client-side route (`/jobs/:id` etc.) has no server
    // handler; the fallback must serve the SPA shell so a deep-link /
    // refresh boots the app instead of 404ing. Only `/api/*` routes
    // are real server endpoints.
    let (_d, app) = fresh_app();
    let (status, ct, body) = fetch(app, "/jobs/01ARZ3NDEKTSV4RRFFQ69G5FAV").await;
    assert_eq!(status, 200);
    assert!(ct.starts_with("text/html"), "wrong content-type: {ct}");
    assert!(!body.is_empty(), "empty shell body");
}

#[tokio::test]
async fn api_jobs_empty_db_is_empty_page() {
    let (_d, app) = fresh_app();
    let (status, ct, body) = fetch(app, "/api/jobs").await;
    assert_eq!(status, 200);
    assert!(
        ct.starts_with("application/json"),
        "wrong content-type: {ct}"
    );
    let page: Page<JobListItem> = serde_json::from_str(&body).expect("Page<JobListItem> JSON");
    assert_eq!(page.total, 0, "empty DB should have no jobs");
    assert!(page.items.is_empty());
    assert_eq!(page.page, 1);
}

#[tokio::test]
async fn api_jobs_get_invalid_id_is_400() {
    let (_d, app) = fresh_app();
    let (status, _ct, body) = fetch(app, "/api/jobs/not-a-ulid").await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert!(body.contains("invalid job id"), "got: {body}");
}

#[tokio::test]
async fn api_jobs_get_missing_id_is_404() {
    let (_d, app) = fresh_app();
    let (status, _ct, body) = fetch(app, "/api/jobs/01ARZ3NDEKTSV4RRFFQ69G5FAV").await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    assert!(body.contains("no such job"), "got: {body}");
}

#[tokio::test]
async fn api_jobs_log_invalid_id_is_400() {
    let (_d, app) = fresh_app();
    let (status, _ct, body) = fetch(app, "/api/jobs/not-a-ulid/log").await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert!(body.contains("invalid job id"), "got: {body}");
}

#[tokio::test]
async fn api_boards_empty_db_is_empty_page() {
    let (_d, app) = fresh_app();
    let (status, ct, body) = fetch(app, "/api/boards").await;
    assert_eq!(status, 200);
    assert!(
        ct.starts_with("application/json"),
        "wrong content-type: {ct}"
    );
    let page: Page<BoardView> = serde_json::from_str(&body).expect("Page<BoardView> JSON");
    assert_eq!(page.total, 0);
    assert!(page.items.is_empty());
}

#[tokio::test]
async fn api_schedules_empty_db_is_empty_page() {
    let (_d, app) = fresh_app();
    let (status, ct, body) = fetch(app, "/api/schedules").await;
    assert_eq!(status, 200);
    assert!(
        ct.starts_with("application/json"),
        "wrong content-type: {ct}"
    );
    let page: Page<ScheduleView> = serde_json::from_str(&body).expect("Page<ScheduleView> JSON");
    assert_eq!(page.total, 0);
    assert!(page.items.is_empty());
}

#[tokio::test]
async fn sse_proxy_rejects_invalid_job_id_with_400() {
    // The SSE proxy's first-line defence: any URL segment that doesn't
    // parse as a `paavo_proto::JobId` (ULID) returns 400 verbatim,
    // without dialling paavod.
    let (_d, app) = fresh_app();
    let (status, _ct, body) = fetch(app, "/api/jobs/not-a-ulid/stream").await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert!(body.contains("invalid job id"), "got: {body}");
}

#[tokio::test]
async fn sse_proxy_returns_502_when_paavod_unreachable() {
    // 127.0.0.1:1 is unambiguously connect-refused on every platform we
    // care about; a reqwest connect error must surface as 502 (paavod's
    // fault), not 500 (paavo-web's) and not a panic.
    let (_d, app) = fresh_app();
    let (status, _ct, body) = fetch(app, "/api/jobs/01ARZ3NDEKTSV4RRFFQ69G5FAV/stream").await;
    assert_eq!(status, axum::http::StatusCode::BAD_GATEWAY);
    assert!(body.contains("paavod unreachable"), "got: {body}");
}
