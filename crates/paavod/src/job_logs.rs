//! Per-job broadcast of log frames + terminal marker. In-memory only;
//! historical frames live in sqlite.
//!
//! Subscriber semantics: a subscriber that joins AFTER a terminal event
//! has been published and the channel finalized will miss the live
//! terminal event. The `stream_job` handler therefore subscribes BEFORE
//! reading the DB; if the DB then shows the job is already terminal the
//! handler emits the terminal line from the persisted outcome and drops
//! the subscriber. The historical frames + terminal-from-DB path is the
//! authoritative "race-free" branch.
//!
//! Lag: the broadcast channel capacity is 256. A slow subscriber that
//! falls behind gets `RecvError::Lagged(n)`; the `stream_job` handler
//! surfaces that as a `{"type":"lagged","missed":n}` NDJSON line so the
//! client can decide whether to refetch from sqlite.

use paavo_proto::{JobId, JobOutcome, LogFrame};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// One streamable event on the live channel.
#[derive(Debug, Clone)]
pub enum LiveEvent {
    /// One log frame.
    Frame(LogFrame),
    /// Terminal outcome — the stream closes after emitting this.
    Terminal(JobOutcome),
}

/// Per-job broadcaster.
#[derive(Clone, Default)]
pub struct JobLogsBroker {
    inner: Arc<Mutex<HashMap<JobId, broadcast::Sender<LiveEvent>>>>,
}

impl JobLogsBroker {
    /// Construct an empty broker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe to (or create) the channel for `id`. Capacity 256.
    pub fn subscribe(&self, id: JobId) -> broadcast::Receiver<LiveEvent> {
        let mut map = self.inner.lock();
        let sender = map
            .entry(id)
            .or_insert_with(|| broadcast::channel(256).0)
            .clone();
        sender.subscribe()
    }

    /// Publish an event to all current subscribers. Returns the number
    /// of subscribers reached (0 means nobody was listening). Lossless
    /// if every subscriber has capacity; lagged subscribers see
    /// `RecvError::Lagged(n)`.
    pub fn publish(&self, id: JobId, event: LiveEvent) -> usize {
        let map = self.inner.lock();
        map.get(&id).and_then(|s| s.send(event).ok()).unwrap_or(0)
    }

    /// Drop the channel after a terminal event has been published so
    /// memory doesn't grow unbounded. Idempotent.
    pub fn finalize(&self, id: JobId) {
        self.inner.lock().remove(&id);
    }

    /// Active-channel count. Useful for tests.
    pub fn active_channels(&self) -> usize {
        self.inner.lock().len()
    }
}
