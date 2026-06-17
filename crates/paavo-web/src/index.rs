//! Live revision poller + the shared [`LiveState`] it feeds.
//!
//! `paavo-web` keeps no per-job data resident: the jobs list and fuzzy
//! search read SQLite directly (see `crate::api::jobs` and
//! `paavo_db::JobRow::search_index_page`). This module's only job is to
//! watch a handful of cheap, bounded aggregates and bump a per-resource
//! revision counter when something changes, which fans out over
//! `/api/events` so the SPA refetches. The `RwLock` guards are always
//! dropped before `.await`.
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// Monotonic per-resource revision counters. Bumped when the resource's
/// content fingerprint changes; pushed to clients over `/api/events`.
#[derive(Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Revisions {
    /// Jobs revision.
    pub jobs: u64,
    /// Boards revision.
    pub boards: u64,
    /// Schedules revision.
    pub schedules: u64,
}

/// Process-local live state shared by the poller and the API handlers: the
/// current revisions and a watch channel that fans revision bumps out to
/// every `/api/events` connection.
#[derive(Clone)]
pub struct LiveState {
    rev: Arc<RwLock<Revisions>>,
    tx: Arc<watch::Sender<Revisions>>,
    fp: Arc<RwLock<(u64, u64, u64)>>, // (jobs, boards, schedules) fingerprints
}

impl Default for LiveState {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveState {
    /// Construct empty live state seeded at revision 0.
    pub fn new() -> Self {
        let (tx, _) = watch::channel(Revisions::default());
        Self {
            rev: Arc::new(RwLock::new(Revisions::default())),
            tx: Arc::new(tx),
            fp: Arc::new(RwLock::new((0, 0, 0))),
        }
    }

    /// A receiver positioned at the current revisions.
    pub fn subscribe(&self) -> watch::Receiver<Revisions> {
        self.tx.subscribe()
    }

    /// Snapshot of the current revisions.
    pub fn revisions(&self) -> Revisions {
        *self.rev.read()
    }
}

fn hash_u64<T: std::hash::Hash>(items: &[T]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    items.len().hash(&mut h);
    for it in items {
        it.hash(&mut h);
    }
    h.finish()
}

/// Spawn the single background poller. `interval` is a parameter so tests
/// can run it at ~20ms. A transient DB read error keeps the last snapshot
/// and skips the tick. The RwLock guards are always dropped before `.await`.
pub fn spawn_poller(db: crate::db::WebDb, live: LiveState, interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            let mut changed = false;
            let mut snap = live.revisions();

            // Jobs: a bounded GROUP BY state digest (no full-table scan into
            // memory). Catches inserts + state transitions.
            if let Ok(digest) = db.jobs_activity_digest() {
                if live.fp.read().0 != digest {
                    live.fp.write().0 = digest;
                    snap.jobs += 1;
                    changed = true;
                }
            }
            if let Ok(boards) = db.all_boards() {
                let keys: Vec<String> = boards
                    .iter()
                    .map(|b| format!("{}:{:?}:{:?}", b.spec.id, b.spec.health, b.last_used_at))
                    .collect();
                let f = hash_u64(&keys);
                if live.fp.read().1 != f {
                    live.fp.write().1 = f;
                    snap.boards += 1;
                    changed = true;
                }
            }
            if let Ok(scheds) = db.all_schedules() {
                let keys: Vec<String> = scheds
                    .iter()
                    .map(|s| {
                        format!(
                            "{}:{}:{:?}:{:?}",
                            s.id, s.enabled, s.last_triggered_at, s.last_completed_at
                        )
                    })
                    .collect();
                let f = hash_u64(&keys);
                if live.fp.read().2 != f {
                    live.fp.write().2 = f;
                    snap.schedules += 1;
                    changed = true;
                }
            }
            if changed {
                *live.rev.write() = snap;
                let _ = live.tx.send(snap);
            }
        }
    });
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

    /// A RW `Db` seeds the same temp file the RO `WebDb` reads via WAL.
    /// After an insert the poller must bump the `jobs` revision, observed by
    /// a `subscribe()`r within a bounded wait.
    #[tokio::test]
    async fn spawn_poller_bumps_jobs_revision_after_insert() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paavo.sqlite");
        let rw = Db::open(&path).unwrap(); // keep the writer alive for WAL visibility
        let webdb = crate::db::WebDb::open(&path).unwrap();
        let live = LiveState::new();
        spawn_poller(webdb, live.clone(), Duration::from_millis(20));
        let mut rx = live.subscribe();

        let id = JobId::new();
        paavo_db::JobRow::insert(rw.raw_conn(), &sample_new_job(id), 0).unwrap();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "poller never bumped the jobs revision"
            );
            if tokio::time::timeout(remaining, rx.changed()).await.is_err() {
                panic!("poller never bumped the jobs revision (timeout)");
            }
            let _ = rx.borrow_and_update();
            if live.revisions().jobs >= 1 {
                break;
            }
        }
    }
}
