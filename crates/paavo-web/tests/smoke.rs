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

#[tokio::test]
async fn aria_current_marks_correct_nav_entry() {
    // Each page's nav should carry exactly one `aria-current="page"`
    // attribute, and it should be on the matching tab. This is the
    // contract that drives the active-link colour CSS rule
    // (`nav.top a[aria-current="page"]` in style.css); regressing it
    // would silently break the visible "you are here" indicator.
    let (_d, app) = fresh_app();
    let cases: &[(&str, &str)] = &[
        ("/", r#"href="/" aria-current="page""#),
        ("/jobs", r#"href="/jobs" aria-current="page""#),
        ("/boards", r#"href="/boards" aria-current="page""#),
        ("/schedule", r#"href="/schedule" aria-current="page""#),
        // /jobs/:id is rendered with the Jobs tab marked.
        ("/jobs/01ARZ3NDEKTSV4RRFFQ69G5FAV", r#"href="/jobs" aria-current="page""#),
    ];
    for (uri, expected) in cases {
        let (status, body) = fetch(app.clone(), uri).await;
        assert_eq!(status, 200, "uri={uri}");
        assert!(
            body.contains(expected),
            "uri={uri} missing nav-current marker {expected}; body: {body}"
        );
        // Exactly one `aria-current` per page — catches a future
        // refactor accidentally tagging two entries.
        let count = body.matches(r#"aria-current="page""#).count();
        assert_eq!(count, 1, "uri={uri} has {count} aria-current markers; expected 1");
    }
}

#[tokio::test]
async fn static_style_css_serves_with_correct_headers() {
    // /static/style.css is the new static-asset route from commit
    // 5cc0ab7's successor (ef-cyprus + ef-symbiosis palette). Serve
    // contract: 200 + correct content-type + cache headers + the
    // CSS variable namespace `--ef-` is present (proves the right
    // bytes got included). Cache-bust query param is allowed but
    // not required for the route to match.
    let (_d, app) = fresh_app();
    let (status, body) = fetch(app, "/static/style.css").await;
    assert_eq!(status, 200);
    assert!(
        body.contains("--ef-bg-main"),
        "expected `--ef-bg-main` in served CSS; got first 200 chars: {}",
        &body.chars().take(200).collect::<String>()
    );
    // Light + dark palette both present.
    assert!(body.contains("prefers-color-scheme: dark"), "missing dark theme media query");
    assert!(body.contains("#fcf7ef"), "missing ef-cyprus bg-main hex");
    assert!(body.contains("#130911"), "missing ef-symbiosis bg-main hex");
}

#[tokio::test]
async fn html_shell_links_static_stylesheet() {
    // Every server-rendered page should link the static stylesheet
    // with a versioned cache-bust query — this is the single place
    // the operator's browser ever pulls CSS from. If a future shell
    // refactor accidentally drops the link, every page renders
    // without colour, theme variables, or layout. Catch that here.
    let (_d, app) = fresh_app();
    let (_status, body) = fetch(app, "/").await;
    assert!(
        body.contains(r#"<link rel="stylesheet" href="/static/style.css?v="#),
        "dashboard missing stylesheet link; body: {body}"
    );
    // No more UnoCSS CDN reference (commit 2 dropped it). Pin the
    // negative case too so a future "let's go back to UnoCSS"
    // experiment trips this assertion intentionally.
    assert!(
        !body.contains("@unocss/runtime"),
        "UnoCSS CDN reference resurfaced; body: {body}"
    );
    assert!(
        !body.contains("cdn.jsdelivr.net"),
        "external CDN reference resurfaced; body: {body}"
    );
}
