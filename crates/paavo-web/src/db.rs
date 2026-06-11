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

    /// All schedule rows.
    pub fn all_schedules(&self) -> paavo_db::Result<Vec<ScheduleRow>> {
        ScheduleRow::list_all(self.inner.lock().raw_conn())
    }
}
