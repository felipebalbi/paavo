//! Integration tests for the consolidated live SSE at `/api/events`.
//!
//! The events stream never closes on its own, so (like the old
//! dashboard-feed tests) these drive the response body incrementally
//! with a timeout rather than `to_bytes`. The flow under test: an
//! immediate `snapshot` on connect, then one named `jobs` event when
//! the background poller observes a newly-inserted row.

use axum::body::Body;
use axum::http::Request;
use paavo_db::{Db, NewJob};
use paavo_proto::{BoardSelector, JobId, JobSource, Priority};
use paavo_web::db::WebDb;
use paavo_web::index::LiveState;
use paavo_web::proxy::{AppState, PaavodClient};
use std::time::Duration;
use tempfile::tempdir;
use tower::ServiceExt;

/// Build an events-enabled paavo-web router over a fresh temp DB.
/// Returns the live RW `Db` writer (keep it alive — it provides WAL
/// visibility for the RO reader and lets a test insert mid-stream), the
/// `TempDir` guard, the `LiveState` (so a test can wait for the poller
/// to settle), and the Router. Spawns the poller at `interval`.
fn events_app(interval: Duration) -> (tempfile::TempDir, Db, LiveState, axum::Router) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let rw = Db::open(&path).unwrap();
    let webdb = WebDb::open(&path).unwrap();
    let live = LiveState::new();
    paavo_web::index::spawn_poller(webdb.clone(), live.clone(), interval);
    let paavod = PaavodClient::new("http://127.0.0.1:1").expect("valid URL");
    let state = AppState {
        db: webdb,
        paavod,
        live: live.clone(),
    };
    let app = paavo_web::app::build_router(state);
    (dir, rw, live, app)
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

/// Wait until the poller has bumped the jobs revision to at least
/// `min`, or panic after `timeout`. On an empty DB the poller's first
/// tick fingerprints all three resources (0 → 1), so waiting for
/// `jobs >= 1` proves the initial settle is done and no spurious delta
/// is still pending — letting the post-insert test observe *only* the
/// insert-driven bump.
async fn wait_for_jobs_revision(live: &LiveState, min: u64, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if live.revisions().jobs >= min {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "jobs revision never reached {min}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn events_emits_initial_snapshot() {
    let (_dir, _rw, live, app) = events_app(Duration::from_millis(20));
    // Let the initial empty-DB fingerprint settle so the snapshot is
    // stable.
    wait_for_jobs_revision(&live, 1, Duration::from_secs(5)).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/events")
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
    let acc = read_until(&mut stream, "event: snapshot", Duration::from_secs(5)).await;
    assert!(
        acc.contains("event: snapshot"),
        "missing snapshot event; got:\n{acc}"
    );
    // The snapshot payload is the Revisions JSON.
    assert!(
        acc.contains("\"jobs\":"),
        "snapshot missing jobs revision; got:\n{acc}"
    );
}

#[tokio::test]
async fn events_pushes_jobs_delta_after_insert() {
    let (_dir, rw, live, app) = events_app(Duration::from_millis(20));
    // Settle the initial empty-DB fingerprint BEFORE connecting, so the
    // only `jobs` delta the stream can emit is the one our insert drives.
    wait_for_jobs_revision(&live, 1, Duration::from_secs(5)).await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let mut stream = resp.into_body().into_data_stream();

    // 1. Drain the initial snapshot.
    let snap = read_until(&mut stream, "event: snapshot", Duration::from_secs(5)).await;
    assert!(
        snap.contains("event: snapshot"),
        "initial snapshot missing; got:\n{snap}"
    );

    // 2. Insert a job via the live RW writer; the poller must observe it
    //    and bump the jobs revision, which the handler emits as a
    //    `jobs` event.
    let id = JobId::new();
    paavo_db::JobRow::insert(rw.raw_conn(), &sample_new_job(id), 0).unwrap();

    // 3. Read until a `jobs` delta arrives.
    let upd = read_until(&mut stream, "event: jobs", Duration::from_secs(5)).await;
    assert!(
        upd.contains("event: jobs"),
        "events did not push a jobs delta after insert; got:\n{upd}"
    );
}
