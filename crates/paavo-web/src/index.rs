//! In-memory jobs index: lightweight rows + fuzzy search, refreshed by
//! the background poller. The index is the single observation point for
//! the live jobs feed and the search endpoint.
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use paavo_proto::JobListItem;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// In-memory jobs index: list items plus a lowercased fuzzy haystack each.
#[derive(Default, Clone)]
pub struct JobIndex {
    items: Vec<JobListItem>,
    haystacks: Vec<String>,
}

impl JobIndex {
    /// Build an index from newest-first list items.
    pub fn from_items(items: Vec<JobListItem>) -> Self {
        let haystacks = items.iter().map(haystack).collect();
        Self { items, haystacks }
    }

    /// Return `(page_items, total)`. Blank `q` => time-ordered (the items
    /// are already newest-first), optionally pinned to `submitted_at <= as_of`.
    /// Non-blank `q` => fuzzy-ranked (score desc, newest-first tiebreak).
    pub fn search(
        &self,
        q: &str,
        as_of: Option<i64>,
        page: u32,
        per_page: u32,
    ) -> (Vec<JobListItem>, u64) {
        let matched: Vec<&JobListItem> = if q.trim().is_empty() {
            self.items
                .iter()
                .filter(|it| as_of.is_none_or(|t| it.submitted_at <= t))
                .collect()
        } else {
            let m = SkimMatcherV2::default();
            let mut scored: Vec<(i64, usize, &JobListItem)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| m.fuzzy_match(&self.haystacks[i], q).map(|s| (s, i, it)))
                .collect();
            // score desc; stable tiebreak by original index (already newest-first)
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            scored.into_iter().map(|(_, _, it)| it).collect()
        };
        let total = matched.len() as u64;
        let start = (page.saturating_sub(1) as usize) * per_page as usize;
        let items = matched
            .into_iter()
            .skip(start)
            .take(per_page as usize)
            .cloned()
            .collect();
        (items, total)
    }

    /// Count of jobs newer than `as_of` (drives the "N new" pill). 0 when
    /// `as_of` is None.
    pub fn new_count(&self, as_of: Option<i64>) -> u64 {
        match as_of {
            Some(t) => self.items.iter().filter(|it| it.submitted_at > t).count() as u64,
            None => 0,
        }
    }
}

fn haystack(it: &JobListItem) -> String {
    format!(
        "{} {} {:?} {}",
        it.id,
        it.submitter,
        it.state,
        it.board_id.as_deref().unwrap_or("")
    )
    .to_lowercase()
}

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

/// Process-local live state shared by the poller and the API handlers:
/// the current jobs index, the current revisions, and a watch channel
/// that fans revision bumps out to every `/api/events` connection.
#[derive(Clone)]
pub struct LiveState {
    /// Current jobs index (rebuilt each time jobs change).
    pub index: Arc<RwLock<JobIndex>>,
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
            index: Arc::new(RwLock::new(JobIndex::default())),
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

fn fp_jobs(items: &[JobListItem]) -> u64 {
    // hash id + state per row (state changes must bump even at constant len)
    let keys: Vec<String> = items
        .iter()
        .map(|it| format!("{}:{:?}", it.id, it.state))
        .collect();
    hash_u64(&keys)
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

            if let Ok(items) = db.jobs_index() {
                let f = fp_jobs(&items);
                if live.fp.read().0 != f {
                    *live.index.write() = JobIndex::from_items(items);
                    live.fp.write().0 = f;
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
    use paavo_proto::{BoardSelector, JobId, JobSource, JobState, Priority};
    use tempfile::tempdir;

    fn item(submitter: &str, board: &str, state: JobState, submitted_at: i64) -> JobListItem {
        JobListItem {
            id: JobId::new(),
            state,
            priority: Priority::Interactive,
            submitter: submitter.into(),
            board_id: Some(board.into()),
            submitted_at,
        }
    }

    /// Newest-first fixture: alice (newest) / bob / cron (oldest).
    fn sample_index() -> JobIndex {
        JobIndex::from_items(vec![
            item("alice", "mcxa266-01", JobState::Running, 3_000),
            item("bob", "mcxa266-02", JobState::Passed, 2_000),
            item("cron", "mcxa266-03", JobState::Building, 1_000),
        ])
    }

    #[test]
    fn blank_query_returns_all_in_order() {
        let idx = sample_index();
        let (items, total) = idx.search("", None, 1, 50);
        assert_eq!(total, 3);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].submitter, "alice");
        assert_eq!(items[2].submitter, "cron");
    }

    #[test]
    fn fuzzy_query_ranks_match_first() {
        let idx = sample_index();
        let (items, total) = idx.search("alice", None, 1, 50);
        assert_eq!(total, 1);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].submitter, "alice");
    }

    #[test]
    fn blank_query_pages() {
        let idx = sample_index();
        // page 2 of size 2 over 3 items => the single remaining (oldest) row.
        let (items, total) = idx.search("", None, 2, 2);
        assert_eq!(total, 3);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].submitter, "cron");
    }

    #[test]
    fn new_count_counts_strictly_newer() {
        let idx = sample_index();
        assert_eq!(idx.new_count(None), 0);
        assert_eq!(idx.new_count(Some(3_000)), 0, "boundary is exclusive");
        assert_eq!(idx.new_count(Some(2_000)), 1);
        assert_eq!(idx.new_count(Some(0)), 3);
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

    /// Mirrors `feed.rs::spawn_poller_pushes_after_insert`: a RW `Db` seeds
    /// the same temp file the RO `WebDb` reads via WAL. After an insert the
    /// poller must bump the `jobs` revision and carry the new row into the
    /// in-memory index, observed by a `subscribe()`r within a bounded wait.
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

        // Loop until a revision bump carries the inserted job into the index.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!remaining.is_zero(), "poller never observed the new job");
            if tokio::time::timeout(remaining, rx.changed()).await.is_err() {
                panic!("poller never observed the new job (timeout)");
            }
            let _ = rx.borrow_and_update();
            // Read+drop the index guard before looping back to the await.
            let (items, _) = live.index.read().search("", None, 1, 100);
            if items.iter().any(|it| it.id == id) {
                break;
            }
        }
        assert!(
            live.revisions().jobs >= 1,
            "jobs revision should have bumped"
        );
    }
}
