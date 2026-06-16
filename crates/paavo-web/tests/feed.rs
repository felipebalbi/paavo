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
    assert!(
        ct.starts_with("text/event-stream"),
        "wrong content-type: {ct}"
    );
    let mut stream = resp.into_body().into_data_stream();
    let acc = read_until(&mut stream, "no jobs yet", Duration::from_secs(5)).await;
    assert!(
        acc.contains("event: recent-jobs"),
        "missing event name; got:\n{acc}"
    );
    assert!(
        acc.contains("no jobs yet"),
        "missing empty-state snapshot; got:\n{acc}"
    );
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
