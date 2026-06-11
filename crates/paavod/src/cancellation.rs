//! Per-job cancel-signal registry. The dispatch loop registers a
//! `crossbeam_channel::Sender<RunCommand>` keyed by `JobId` when it
//! launches a BoardWorker; the cancel handler signals through it to
//! satisfy `POST /jobs/:id/cancel` while the job is Building or
//! Running. The watchdog inside the worker maps `Cancel` /
//! `DaemonShutdown` to the right outcome variant per spec §5.4.
//!
//! v1 semantics: a second `register(id, ...)` for the same job
//! overwrites the previous Sender silently (there is exactly one
//! dispatch loop, so a duplicate is "the authoritative new one").
//! Tests pin this so a future contributor can decide to upgrade to a
//! panic if the single-writer invariant changes.

use crossbeam_channel::Sender;
use paavo_proto::JobId;
use paavo_runner::RunCommand;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Cancel signal handle keyed by job id.
#[derive(Clone, Default)]
pub struct CancellationRegistry {
    inner: Arc<Mutex<HashMap<JobId, Sender<RunCommand>>>>,
}

impl CancellationRegistry {
    /// Register a fresh sender for a job that's about to run. If
    /// `id` already has a sender, the old one is dropped silently.
    pub fn register(&self, id: JobId, tx: Sender<RunCommand>) {
        self.inner.lock().insert(id, tx);
    }

    /// Drop the sender after the job has finalized. Idempotent.
    pub fn unregister(&self, id: &JobId) {
        self.inner.lock().remove(id);
    }

    /// Try to send a `Cancel` / `DaemonShutdown` to a running job.
    /// Returns `true` only if a sender existed AND the send succeeded
    /// (the receiver was still alive). A dropped receiver (worker
    /// exited) returns `false` so the HTTP layer can fall through to
    /// 409.
    pub fn signal(&self, id: &JobId, cmd: RunCommand) -> bool {
        match self.inner.lock().get(id) {
            Some(tx) => tx.send(cmd).is_ok(),
            None => false,
        }
    }

    /// Signal every registered job. Used during shutdown drain
    /// (M4.3.d). Send errors are swallowed — a dropped receiver during
    /// shutdown is fine.
    pub fn signal_all(&self, cmd: RunCommand) {
        for (_id, tx) in self.inner.lock().iter() {
            let _ = tx.send(cmd);
        }
    }

    /// Active-entry count. Useful for tests and drain bookkeeping.
    pub fn active(&self) -> usize {
        self.inner.lock().len()
    }
}
