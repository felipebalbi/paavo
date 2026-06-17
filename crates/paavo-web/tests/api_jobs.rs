//! Integration tests for `GET /api/jobs`: pagination + fuzzy search.
//!
//! Rows are seeded via a live RW `Db` writer and read back through the
//! read-only `WebDb` (WAL), which the handler now queries directly — there
//! is no in-memory index. A short poll loop tolerates WAL read-visibility
//! latency before asserting.

use axum::body::{to_bytes, Body};
use axum::http::Request;
use paavo_db::{Db, JobRow, NewJob};
use paavo_proto::{BoardSelector, JobId, JobListItem, JobSource, Page, Priority};
use paavo_web::db::WebDb;
use paavo_web::index::LiveState;
use paavo_web::proxy::{AppState, PaavodClient};
use std::time::Duration;
use tempfile::tempdir;
use tower::ServiceExt;

/// Build a paavo-web router with a spawned poller over a fresh temp DB.
/// Returns the live RW `Db` writer (keep it alive for WAL visibility +
/// mid-test inserts), the `TempDir` guard, and the Router.
fn jobs_app(interval: Duration) -> (tempfile::TempDir, Db, axum::Router) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let rw = Db::open(&path).unwrap();
    let webdb = WebDb::open(&path).unwrap();
    // The poller drives only the live `jobs` revision now; `GET /api/jobs`
    // reads SQLite directly. The shared `LiveState` still supplies the
    // revision echoed on each page.
    let live = LiveState::new();
    paavo_web::index::spawn_poller(webdb.clone(), live.clone(), interval);
    let paavod = PaavodClient::new("http://127.0.0.1:1").expect("valid URL");
    let state = AppState {
        db: webdb,
        paavod,
        live,
    };
    let app = paavo_web::app::build_router(state);
    (dir, rw, app)
}

fn new_job(id: JobId, submitter: &str) -> NewJob {
    NewJob {
        id,
        priority: Priority::Interactive,
        submitter: submitter.into(),
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

async fn get_page(app: &axum::Router, uri: &str) -> Page<JobListItem> {
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "GET {uri} not 200");
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("Page<JobListItem> JSON")
}

/// Poll `GET /api/jobs` until it reports `want` total rows (WAL read
/// visibility is effectively immediate; the loop guards against any
/// checkpoint lag) or the timeout elapses.
async fn wait_for_total(app: &axum::Router, want: u64, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let page = get_page(app, "/api/jobs?per_page=100").await;
        if page.total == want {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "index never reached {want} jobs (last total {})",
            page.total
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn jobs_list_paginates_and_searches() {
    let (_dir, rw, app) = jobs_app(Duration::from_millis(20));

    // Seed three jobs with distinct submitters. Fuzzy `q=alice` will
    // match only the alice row: ULIDs use Crockford base32 (no i/l) and
    // neither "bob" nor "carol" contains a,l,i,c,e in order.
    for submitter in ["alice", "bob", "carol"] {
        JobRow::insert(rw.raw_conn(), &new_job(JobId::new(), submitter), 0).unwrap();
    }
    wait_for_total(&app, 3, Duration::from_secs(5)).await;

    // Page 1 of size 2 over 3 rows: 2 items, total still 3.
    let p1 = get_page(&app, "/api/jobs?per_page=2&page=1").await;
    assert_eq!(p1.total, 3);
    assert_eq!(p1.items.len(), 2);
    assert_eq!(p1.page, 1);
    assert_eq!(p1.per_page, 2);

    // Page 2 of size 2: the single remaining row.
    let p2 = get_page(&app, "/api/jobs?per_page=2&page=2").await;
    assert_eq!(p2.total, 3);
    assert_eq!(p2.items.len(), 1);
    assert_eq!(p2.page, 2);

    // Fuzzy search narrows to exactly the alice row.
    let q = get_page(&app, "/api/jobs?q=alice").await;
    assert_eq!(q.total, 1, "q=alice should match exactly one row");
    assert_eq!(q.items.len(), 1);
    assert_eq!(q.items[0].submitter, "alice");
}

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
