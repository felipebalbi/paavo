//! Watchdog thread: tick every 100 ms, fire Cancel if either inactivity or
//! hard-max exceeded, or if the cancel channel produces a command.

use crate::job::RunCommand;
use crossbeam_channel::Receiver;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Reason the watchdog signalled a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// `now - last_activity > inactivity`.
    Inactivity,
    /// `now - start > hard_max`.
    HardMax,
    /// External `RunCommand::Cancel`.
    UserCancel,
    /// External `RunCommand::DaemonShutdown`.
    DaemonShutdown,
}

/// Shared mutable state between the BoardWorker and its Watchdog.
pub struct WatchdogState {
    /// Wall-clock instant of the most recent `Event::LogFrame` (terminal
    /// events do not bump this clock — they end the worker loop directly).
    /// Updated by the worker via `touch()` on every decoded defmt frame.
    pub(crate) last_activity: Mutex<Instant>,
    /// Wall-clock instant the job started.
    pub(crate) started_at: Instant,
    /// Set by the watchdog when it has fired; the worker checks this each
    /// time it considers another `next_event` call.
    pub(crate) stop_reason: Mutex<Option<StopReason>>,
}

impl WatchdogState {
    /// Construct fresh state.
    pub fn new(now: Instant) -> Arc<Self> {
        Arc::new(Self {
            last_activity: Mutex::new(now),
            started_at: now,
            stop_reason: Mutex::new(None),
        })
    }

    /// Worker bumps this on every event observed.
    pub fn touch(&self, now: Instant) {
        *self.last_activity.lock() = now;
    }

    /// Worker polls this to decide whether to break its loop.
    pub fn stop_reason(&self) -> Option<StopReason> {
        *self.stop_reason.lock()
    }
}

/// Tick loop. Returns when a stop has been signalled.
///
/// Exits in any of four conditions:
/// 1. `worker_done_rx` produces a unit value → exits silently (the worker
///    reached a natural terminal state and is about to join us). No stop
///    reason is recorded in `state.stop_reason`.
/// 2. `cancel_rx` produces a `RunCommand` → records the corresponding stop
///    reason in `state.stop_reason` and returns.
/// 3. Inactivity exceeded → records `Inactivity` and returns.
/// 4. Hard-max exceeded → records `HardMax` and returns.
///
/// The worker observes the stop via `state.stop_reason()` on its next loop
/// iteration; there is no separate notification channel because the worker
/// is normally inside `session.next_event()` (and so wouldn't receive on a
/// channel anyway) — the mutex-protected shared state is the single source
/// of truth.
pub fn run_watchdog(
    state: Arc<WatchdogState>,
    inactivity: Duration,
    hard_max: Duration,
    cancel_rx: Receiver<RunCommand>,
    tick: Duration,
    worker_done_rx: Receiver<()>,
) {
    loop {
        // Worker reached a natural terminal state → exit silently.
        if worker_done_rx.try_recv().is_ok() {
            return;
        }
        // External signal?
        if let Ok(cmd) = cancel_rx.try_recv() {
            let reason = match cmd {
                RunCommand::Cancel => StopReason::UserCancel,
                RunCommand::DaemonShutdown => StopReason::DaemonShutdown,
            };
            *state.stop_reason.lock() = Some(reason);
            return;
        }
        let now = Instant::now();
        let elapsed_total = now.duration_since(state.started_at);
        let elapsed_silent = now.duration_since(*state.last_activity.lock());
        if elapsed_silent > inactivity {
            *state.stop_reason.lock() = Some(StopReason::Inactivity);
            return;
        }
        if elapsed_total > hard_max {
            *state.stop_reason.lock() = Some(StopReason::HardMax);
            return;
        }
        std::thread::sleep(tick);
    }
}
