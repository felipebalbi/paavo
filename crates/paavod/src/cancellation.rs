//! Per-job cancel-signal registry. The dispatch loop calls
//! `register(id)` to allocate a fresh `crossbeam_channel` keyed by
//! `JobId` when it claims a job; the cancel handler calls
//! `signal(id, ...)` to satisfy `POST /jobs/:id/cancel` while the job
//! is Building or Running; the runner calls `take_receiver(id)` to
//! consume the rx half so the BoardWorker's watchdog can read cancel
//! signals directly. The watchdog inside the worker maps `Cancel` /
//! `DaemonShutdown` to the right outcome variant per spec §5.4.
//!
//! The registry stores BOTH halves (`Sender`, `Option<Receiver>`) so
//! `take_receiver` can hand out the rx without invalidating `signal`:
//! the sender stays in the map post-take, so a cancel request that
//! lands after the worker has the rx still routes through correctly.
//!
//! v1 semantics: a second `register(id)` for the same job overwrites
//! the previous (sender, receiver) tuple silently (there is exactly one
//! dispatch loop, so a duplicate is "the authoritative new one").
//! Tests pin this so a future contributor can decide to upgrade to a
//! panic if the single-writer invariant changes.

use crossbeam_channel::{unbounded, Receiver, Sender};
use paavo_proto::JobId;
use paavo_runner::RunCommand;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// One registry entry: the sender stays in place for the lifetime
/// of the job (so `signal()` works even after the runner takes the
/// rx); the receiver is `Some` until `take_receiver` consumes it.
type Entry = (Sender<RunCommand>, Option<Receiver<RunCommand>>);

/// Cancel signal handle keyed by job id.
///
/// Each entry holds `(Sender, Option<Receiver>)`. The receiver is
/// `Some` until `take_receiver` has consumed it; `signal` only needs
/// the sender, so it works before AND after `take_receiver`.
#[derive(Clone, Default)]
pub struct CancellationRegistry {
    inner: Arc<Mutex<HashMap<JobId, Entry>>>,
}

impl CancellationRegistry {
    /// Allocate a fresh `crossbeam_channel` for a job that's about to
    /// run. If `id` already has an entry, the old `(tx, rx)` tuple is
    /// dropped silently (see module docstring for the v1 single-writer
    /// rationale). The rx is held inside the entry until
    /// `take_receiver` consumes it.
    pub fn register(&self, id: JobId) {
        let (tx, rx) = unbounded::<RunCommand>();
        self.inner.lock().insert(id, (tx, Some(rx)));
    }

    /// Take and remove the receiver half — used by `RealRunner` so the
    /// `BoardWorker`'s watchdog can read cancel signals directly.
    ///
    /// The sender stays in the entry so `signal()` keeps working after
    /// `take_receiver`. Returns `None` if no entry exists (dispatch
    /// never called `register` for this job) OR if the receiver was
    /// already taken — the second case shouldn't happen under the
    /// current single-runner contract but is benign (the caller's
    /// fallback path uses a disconnected receiver, which the watchdog
    /// tolerates).
    pub fn take_receiver(&self, id: &JobId) -> Option<Receiver<RunCommand>> {
        self.inner.lock().get_mut(id).and_then(|(_, rx)| rx.take())
    }

    /// Drop the entry after the job has finalized. Idempotent.
    pub fn unregister(&self, id: &JobId) {
        self.inner.lock().remove(id);
    }

    /// Try to send a `Cancel` / `DaemonShutdown` to a running job.
    /// Returns `true` only if an entry existed AND the send succeeded
    /// (the receiver was still alive). A dropped receiver (worker
    /// exited, or `take_receiver` consumer dropped its rx) returns
    /// `false` so the HTTP layer can fall through to 409.
    pub fn signal(&self, id: &JobId, cmd: RunCommand) -> bool {
        match self.inner.lock().get(id) {
            Some((tx, _)) => tx.send(cmd).is_ok(),
            None => false,
        }
    }

    /// Signal every registered job. Used during shutdown drain
    /// (M4.3.d). Send errors are swallowed — a dropped receiver during
    /// shutdown is fine.
    pub fn signal_all(&self, cmd: RunCommand) {
        for (_id, (tx, _)) in self.inner.lock().iter() {
            let _ = tx.send(cmd);
        }
    }

    /// Active-entry count. Useful for tests and drain bookkeeping.
    pub fn active(&self) -> usize {
        self.inner.lock().len()
    }
}

/// Per-job kill switch for the BUILD phase. Separate from
/// `CancellationRegistry` (which carries `RunCommand` to the run
/// watchdog): a build cancel just kills the cargo child via a `()`
/// signal the build task hands to `Builder::build`.
#[derive(Clone, Default)]
pub struct BuildCancelRegistry {
    inner: Arc<Mutex<HashMap<JobId, Sender<()>>>>,
}

impl BuildCancelRegistry {
    /// Allocate a kill channel for a build about to start; returns the rx
    /// the build task passes to `Builder::build`.
    pub fn register(&self, id: JobId) -> Receiver<()> {
        let (tx, rx) = unbounded::<()>();
        self.inner.lock().insert(id, tx);
        rx
    }

    /// Request a kill of the in-flight build. Returns `true` if a live
    /// build channel existed (the receiver was still alive).
    pub fn signal(&self, id: &JobId) -> bool {
        match self.inner.lock().get(id) {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    /// Drop the entry when the build finishes. Idempotent.
    pub fn unregister(&self, id: &JobId) {
        self.inner.lock().remove(id);
    }

    /// In-flight build count (drain bookkeeping + tests).
    pub fn active(&self) -> usize {
        self.inner.lock().len()
    }
}

#[cfg(test)]
mod build_cancel_tests {
    use super::BuildCancelRegistry;
    use paavo_proto::JobId;

    #[test]
    fn register_signal_unregister() {
        let reg = BuildCancelRegistry::default();
        let id = JobId::new();
        let rx = reg.register(id);
        assert_eq!(reg.active(), 1);
        assert!(reg.signal(&id), "signal delivers while rx alive");
        rx.recv().unwrap();
        reg.unregister(&id);
        assert_eq!(reg.active(), 0);
        assert!(!reg.signal(&id), "signal after unregister is false");
    }
}
