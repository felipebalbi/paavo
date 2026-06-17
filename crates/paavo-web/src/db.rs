//! Read-only DB handle + minimal typed queries used by the pages.
//!
//! All decode logic lives in paavo-db's `from_row` family — this
//! module is a thin façade so the web layer never duplicates row
//! decoding or column lists.
//!
//! ## Sync calls in async handlers
//!
//! Every helper here is synchronous (rusqlite has no async surface).
//! Page handlers invoke them directly inside `async fn render(...)`
//! without `spawn_blocking`. This is deliberate: the WAL+RO reader
//! does sub-millisecond queries against a local file, the workload is
//! single-tenant viewer traffic, and the cost of spawning + joining a
//! blocking task would dwarf the query itself. Re-evaluate if a page
//! ever grows a multi-row table-scan or remote-storage backing.

use paavo_db::{BoardRow, Db, JobRow, LogFrameDb, ScheduleRow};
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;

/// Read-only DB façade. Cloneable so axum can put it in router state.
#[derive(Clone)]
pub struct WebDb {
    inner: Arc<Mutex<Db>>,
}

impl WebDb {
    /// Open paavo.sqlite in WAL+RO mode.
    pub fn open(path: &Path) -> paavo_db::Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(Db::open_readonly(path)?)),
        })
    }

    /// All boards, ordered by id.
    pub fn all_boards(&self) -> paavo_db::Result<Vec<BoardRow>> {
        BoardRow::list_all(self.inner.lock().raw_conn())
    }

    /// Most recent `limit` jobs across all states.
    pub fn recent_jobs(&self, limit: u32) -> paavo_db::Result<Vec<JobRow>> {
        JobRow::list_recent(self.inner.lock().raw_conn(), limit)
    }

    /// One job by id.
    pub fn job(&self, id: &paavo_proto::JobId) -> paavo_db::Result<Option<JobRow>> {
        JobRow::find(self.inner.lock().raw_conn(), id)
    }

    /// Up to `limit` log frames for a job, oldest first.
    pub fn job_logs(
        &self,
        id: &paavo_proto::JobId,
        limit: u32,
    ) -> paavo_db::Result<Vec<paavo_proto::LogFrame>> {
        paavo_proto::LogFrame::list(self.inner.lock().raw_conn(), id, 0, limit)
    }

    /// A page of log frames for a job (oldest first), starting at
    /// `offset`. Backs `GET /api/jobs/:id/log`, which the SPA uses to
    /// backfill scrollback before attaching the live SSE tail.
    pub fn job_logs_page(
        &self,
        id: &paavo_proto::JobId,
        offset: u32,
        limit: u32,
    ) -> paavo_db::Result<Vec<paavo_proto::LogFrame>> {
        paavo_proto::LogFrame::list(self.inner.lock().raw_conn(), id, offset, limit)
    }

    /// All schedule rows.
    pub fn all_schedules(&self) -> paavo_db::Result<Vec<ScheduleRow>> {
        ScheduleRow::list_all(self.inner.lock().raw_conn())
    }

    /// One page of fuzzy-search results (lightweight projection), ranked.
    pub fn jobs_search_page(
        &self,
        q: &str,
        offset: u32,
        limit: u32,
    ) -> paavo_db::Result<Vec<paavo_proto::JobListItem>> {
        paavo_db::JobRow::search_index_page(self.inner.lock().raw_conn(), q, offset, limit)
    }

    /// Total fuzzy-search matches for `q` (pagination total).
    pub fn jobs_search_count(&self, q: &str) -> paavo_db::Result<u64> {
        paavo_db::JobRow::search_count(self.inner.lock().raw_conn(), q)
    }

    /// One page of the time-ordered jobs list (lightweight), optionally
    /// pinned to `submitted_at <= as_of`.
    pub fn jobs_list_page(
        &self,
        as_of: Option<i64>,
        offset: u32,
        limit: u32,
    ) -> paavo_db::Result<Vec<paavo_proto::JobListItem>> {
        paavo_db::JobRow::list_index_page(self.inner.lock().raw_conn(), as_of, offset, limit)
    }

    /// Count of jobs newer than `as_of` (the "N new" pill); 0 when unpinned.
    pub fn jobs_new_count(&self, as_of: Option<i64>) -> paavo_db::Result<u64> {
        paavo_db::JobRow::count_newer(self.inner.lock().raw_conn(), as_of)
    }

    /// Bounded change-detection fingerprint for the live poller.
    pub fn jobs_activity_digest(&self) -> paavo_db::Result<u64> {
        paavo_db::JobRow::activity_digest(self.inner.lock().raw_conn())
    }

    /// Page of full job rows, optionally pinned to `submitted_at <= as_of`.
    pub fn jobs_page(
        &self,
        as_of: Option<i64>,
        offset: u32,
        limit: u32,
    ) -> paavo_db::Result<Vec<paavo_db::JobRow>> {
        paavo_db::JobRow::list_page(self.inner.lock().raw_conn(), as_of, offset, limit)
    }

    /// Count of jobs (optionally `submitted_at <= as_of`).
    pub fn jobs_count(&self, as_of: Option<i64>) -> paavo_db::Result<u64> {
        paavo_db::JobRow::count(self.inner.lock().raw_conn(), as_of)
    }

    /// Page of boards (id ASC), optionally narrowed to an `id`/`kind`
    /// substring (see [`paavo_db::BoardRow::list_page`]). The filter is
    /// applied across the whole `board` table server-side, so the SPA's
    /// fleet search finds matches regardless of which page they fall on.
    pub fn boards_page(
        &self,
        filter: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> paavo_db::Result<Vec<paavo_db::BoardRow>> {
        paavo_db::BoardRow::list_page(self.inner.lock().raw_conn(), filter, offset, limit)
    }

    /// Total board count, optionally filtered exactly like
    /// [`Self::boards_page`] so the page count reflects the filtered set.
    pub fn boards_count(&self, filter: Option<&str>) -> paavo_db::Result<u64> {
        paavo_db::BoardRow::count(self.inner.lock().raw_conn(), filter)
    }

    /// Page of schedules (id ASC).
    pub fn schedules_page(
        &self,
        offset: u32,
        limit: u32,
    ) -> paavo_db::Result<Vec<paavo_db::ScheduleRow>> {
        paavo_db::ScheduleRow::list_page(self.inner.lock().raw_conn(), offset, limit)
    }

    /// Total schedule count.
    pub fn schedules_count(&self) -> paavo_db::Result<u64> {
        paavo_db::ScheduleRow::count(self.inner.lock().raw_conn())
    }
}
