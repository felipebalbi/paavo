//! Dashboard live feed: one background poller renders the "Recent jobs"
//! table from the read-only DB and fans it out to connected browsers
//! over SSE. See the design at
//! `docs/superpowers/specs/2026-06-16-paavo-web-live-dashboard-design.md`.

use crate::db::WebDb;
use crate::pages::dashboard::{recent_jobs_tbody, RECENT_JOBS_LIMIT};
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use std::convert::Infallible;
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
        assert!(
            p.contains(&id.to_string()),
            "payload missing job id; got: {p}"
        );
    }

    #[test]
    fn publish_if_changed_only_fires_on_change() {
        let feed = JobFeed::new("a".to_string());
        let mut rx = feed.subscribe();
        // Same value: no change.
        feed.publish_if_changed("a".to_string());
        assert!(
            !rx.has_changed().unwrap(),
            "unchanged publish should not notify"
        );
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
