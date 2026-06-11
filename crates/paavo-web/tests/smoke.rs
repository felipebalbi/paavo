use axum::body::{to_bytes, Body};
use axum::http::Request;
use paavo_db::Db;
use paavo_web::db::WebDb;
use tempfile::tempdir;
use tower::ServiceExt;

/// Helper: open a fresh empty DB and build a router around it. Keeps the
/// `TempDir` alive on the stack so the sqlite file isn't unlinked while
/// the test is still reading it.
fn fresh_app() -> (tempfile::TempDir, axum::Router) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let _ = Db::open(&path).unwrap(); // run migrations
    let db = WebDb::open(&path).unwrap();
    let app = paavo_web::app::build_router(db);
    (dir, app)
}

async fn fetch(app: axum::Router, uri: &str) -> (axum::http::StatusCode, String) {
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn dashboard_renders_on_empty_db() {
    let (_d, app) = fresh_app();
    let (status, body) = fetch(app, "/").await;
    assert_eq!(status, 200);
    assert!(body.contains("Board fleet"), "got: {body}");
    assert!(body.contains("Recent jobs"), "got: {body}");
}

#[tokio::test]
async fn jobs_list_renders_on_empty_db() {
    let (_d, app) = fresh_app();
    let (status, body) = fetch(app, "/jobs").await;
    assert_eq!(status, 200);
    assert!(body.contains("jobs"), "got: {body}");
}

#[tokio::test]
async fn job_detail_invalid_id_renders_invalid_id_body() {
    // Page returns 200 with an "invalid id" body for non-ULID input,
    // not a 404 status — operators following a link from a stale log
    // shouldn't see an unstyled error page. The "invalid id" wording is
    // a plan contract; a future refactor that collapses this branch
    // into "not found" should fail this assertion.
    let (_d, app) = fresh_app();
    let (status, body) = fetch(app, "/jobs/not-a-ulid").await;
    assert_eq!(status, 200);
    assert!(body.contains("invalid id"), "got: {body}");
}

#[tokio::test]
async fn job_detail_well_formed_but_missing_id_renders_not_found() {
    // Well-formed ULID that does not exist in the DB → distinct error
    // wording so operators can tell apart "you typed the id wrong" from
    // "the id is gone (retention swept it)".
    let (_d, app) = fresh_app();
    // 01ARZ3NDEKTSV4RRFFQ69G5FAV is a canonical ULID example from the
    // ulid-spec README — picked deliberately so the test reads as a
    // plausible id rather than an obvious placeholder.
    let (status, body) = fetch(app, "/jobs/01ARZ3NDEKTSV4RRFFQ69G5FAV").await;
    assert_eq!(status, 200);
    assert!(body.contains("not found"), "got: {body}");
}

#[tokio::test]
async fn boards_renders_on_empty_db() {
    let (_d, app) = fresh_app();
    let (status, body) = fetch(app, "/boards").await;
    assert_eq!(status, 200);
    assert!(body.contains("boards"), "got: {body}");
}

#[tokio::test]
async fn schedule_renders_on_empty_db() {
    let (_d, app) = fresh_app();
    let (status, body) = fetch(app, "/schedule").await;
    assert_eq!(status, 200);
    assert!(body.contains("schedule"), "got: {body}");
    assert!(body.contains("no schedules registered yet"), "got: {body}");
}

#[tokio::test]
async fn nav_present_on_every_page() {
    // The sticky nav is shared by html_shell; smoke check the four nav
    // anchors are present on every page render. Both job-detail error
    // branches (invalid + not-found) go through html_shell, so include
    // representative URIs for each — this is the only page with
    // early-return branches that could plausibly bypass the shell.
    let (_d, app) = fresh_app();
    for uri in [
        "/",
        "/jobs",
        "/boards",
        "/schedule",
        "/jobs/not-a-ulid",
        "/jobs/01ARZ3NDEKTSV4RRFFQ69G5FAV",
    ] {
        let (status, body) = fetch(app.clone(), uri).await;
        assert_eq!(status, 200, "uri={uri}");
        for anchor in [
            r#"href="/""#,
            r#"href="/jobs""#,
            r#"href="/boards""#,
            r#"href="/schedule""#,
        ] {
            assert!(
                body.contains(anchor),
                "uri={uri} missing nav anchor {anchor}; body: {body}"
            );
        }
    }
}
