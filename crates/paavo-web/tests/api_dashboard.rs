//! Integration test for `GET /api/dashboard`: SQL aggregate counts +
//! the recent-jobs (in-memory index) and fleet (SQL) display slices.
//!
//! Counts/fleet read sqlite directly (immediate via WAL); recent_jobs is
//! poller-maintained, so the test polls the endpoint until the index
//! reflects the seeded jobs before asserting.

use axum::body::{to_bytes, Body};
use axum::http::Request;
use paavo_db::{BoardRow, Db, JobRow, NewJob};
use paavo_proto::{
    BoardHealth, BoardSelector, BoardSpec, DashboardOverview, JobId, JobSource, Priority,
    ProbeSelector,
};
use paavo_web::db::WebDb;
use paavo_web::index::LiveState;
use paavo_web::proxy::{AppState, PaavodClient};
use std::time::Duration;
use tempfile::tempdir;
use tower::ServiceExt;

fn app(interval: Duration) -> (tempfile::TempDir, Db, axum::Router) {
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
        live,
    };
    let app = paavo_web::app::build_router(state);
    (dir, rw, app)
}

fn board(id: &str) -> BoardSpec {
    BoardSpec {
        id: id.into(),
        kind: "mcxa266".into(),
        probe_selector: ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: "ABC".into(),
        },
        chip_name: "MCXA266VFL".into(),
        target_name: "frdm-mcx-a266".into(),
        wiring_profile: Some("default".into()),
        health: BoardHealth::Healthy,
    }
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

async fn get_overview(app: &axum::Router) -> DashboardOverview {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "GET /api/dashboard not 200");
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("DashboardOverview JSON")
}

/// Poll until the in-memory index carries `want` recent jobs.
async fn wait_for_recent(app: &axum::Router, want: usize) -> DashboardOverview {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let ov = get_overview(app).await;
        if ov.recent_jobs.len() == want {
            return ov;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "index never reached {want} recent jobs (last {})",
            ov.recent_jobs.len()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn dashboard_reports_sql_counts_recent_jobs_and_fleet() {
    let (_dir, rw, app) = app(Duration::from_millis(20));

    // Two boards: one healthy, one quarantined.
    BoardRow::insert(rw.raw_conn(), &board("z-healthy"), 0).unwrap();
    BoardRow::insert(rw.raw_conn(), &board("a-quarantined"), 0).unwrap();
    BoardRow::quarantine(rw.raw_conn(), "a-quarantined", "broken").unwrap();

    // Two submitted jobs; bob is newer than alice.
    JobRow::insert(rw.raw_conn(), &new_job(JobId::new(), "alice"), 1000).unwrap();
    JobRow::insert(rw.raw_conn(), &new_job(JobId::new(), "bob"), 2000).unwrap();

    let ov = wait_for_recent(&app, 2).await;

    // Job state counts (SQL, exact).
    assert_eq!(ov.jobs.submitted, 2);
    assert_eq!(ov.jobs.queue(), 2);
    assert_eq!(ov.jobs.terminal(), 0);
    assert_eq!(ov.jobs.pass_rate_pct(), None);

    // Board health counts (SQL, exact).
    assert_eq!(ov.boards.total, 2);
    assert_eq!(ov.boards.quarantined, 1);
    assert_eq!(ov.boards.healthy(), 1);

    // Recent jobs: newest-first.
    assert_eq!(ov.recent_jobs.len(), 2);
    assert_eq!(ov.recent_jobs[0].submitter, "bob");
    assert_eq!(ov.recent_jobs[1].submitter, "alice");

    // Fleet slice: both boards present, quarantined leads.
    assert_eq!(ov.fleet.len(), 2);
    assert_eq!(ov.fleet[0].spec.id, "a-quarantined");
}

#[tokio::test]
async fn dashboard_counts_are_uncapped_while_recent_jobs_are_capped() {
    let (_dir, rw, app) = app(Duration::from_millis(20));

    // Seed more submitted jobs than the recent-jobs display cap (8).
    for i in 0..10 {
        JobRow::insert(
            rw.raw_conn(),
            &new_job(JobId::new(), &format!("user{i}")),
            1000 + i as i64,
        )
        .unwrap();
    }

    // The in-memory index holds every job but the handler caps the
    // recent-activity slice at RECENT_JOBS (8); poll until it stabilises
    // at 8 (it can never reach 10).
    let ov = wait_for_recent(&app, 8).await;

    // Counts are EXACT, UNCAPPED SQL aggregates over the whole table...
    assert_eq!(ov.jobs.submitted, 10);
    assert_eq!(ov.jobs.queue(), 10);
    // ...while the recent-activity list is the capped index slice.
    assert_eq!(ov.recent_jobs.len(), 8);
}
