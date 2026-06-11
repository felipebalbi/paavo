//! Shared axum state: db handle, config, fleet inventory cache, and the
//! SIGTERM drain flag.
//!
//! Concurrency contract:
//! - `db` uses `parking_lot::Mutex` because every SQLite call is sub-ms
//!   and the daemon is single-host. Lock duration is bounded; never hold
//!   the guard across an `.await`. Handlers that need to do async work
//!   after a read should copy the rows out, drop the guard, then await.
//!   `await_holding_lock` would warn about this if we ever drift.
//! - `inventory` is a write-through cache of the `boards` table. It MUST
//!   be hydrated once at startup by `paavod::main` (see Task 4.4) before
//!   the HTTP server starts accepting requests — otherwise the daemon
//!   will reject every selector after a restart until the operator does
//!   a redundant `POST /boards`. Handlers that mutate boards refresh
//!   the cache under the same lock.
//! - `drain` is a one-shot flag (false → true). M4.3.d wires the SIGTERM
//!   handler that calls `set_draining`.

#![deny(clippy::await_holding_lock)]

use paavo_proto::BoardSpec;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Drain mode for SIGTERM handling. One-way flag (false → true).
#[derive(Debug, Default, Clone)]
pub struct DrainState {
    inner: Arc<AtomicBool>,
}

impl DrainState {
    /// Returns true while the daemon is draining for shutdown.
    pub fn is_draining(&self) -> bool {
        // Acquire pairs with Release in `set_draining`; both writers
        // and readers see a consistent transition.
        self.inner.load(Ordering::Acquire)
    }
    /// Mark drain mode. Idempotent.
    pub fn set_draining(&self) {
        self.inner.store(true, Ordering::Release);
    }
}

/// Shared axum state.
#[derive(Clone)]
pub struct AppState {
    /// Daemon SQLite handle. Locked per-handler via `lock()`; serialised
    /// access — see the concurrency contract in the module docstring.
    pub db: Arc<Mutex<paavo_db::Db>>,
    /// Loaded config (immutable post-load).
    pub config: Arc<crate::config::Config>,
    /// In-memory inventory snapshot. Hydrated by `paavod::main` at
    /// startup; refreshed by every successful `boards` write.
    pub inventory: Arc<Mutex<Vec<BoardSpec>>>,
    /// One-shot SIGTERM drain flag.
    pub drain: DrainState,
    /// Per-job log frame broker; subscribers consume live frames
    /// while a job is Building or Running. See
    /// `crate::job_logs::JobLogsBroker`.
    pub job_logs: crate::job_logs::JobLogsBroker,
    /// Per-job cancel-signal registry; the dispatch loop registers
    /// at job start, the cancel handler signals through it for
    /// `Building` / `Running` jobs, and `paavod::main` calls
    /// `signal_all(DaemonShutdown)` during SIGTERM drain.
    pub cancellation: crate::cancellation::CancellationRegistry,
}

impl AppState {
    /// Take a copy of the current inventory for selector validation.
    pub fn inventory_snapshot(&self) -> Vec<BoardSpec> {
        self.inventory.lock().clone()
    }
}
