//! SIGTERM drain. Two pieces:
//!
//! 1. `wait_for_signal()` — async; resolves on SIGTERM (unix) or
//!    Ctrl-C (any platform). Wired into `paavod::main`'s
//!    `axum::serve(...).with_graceful_shutdown(...)` call.
//! 2. `drain_with_grace(state, cron, grace)` — pure async logic
//!    that:
//!    a. flips `state.drain`. From this point: `POST /jobs` returns
//!    503, the dispatch loop stops picking new work, the cron's
//!    per-fire body short-circuits.
//!    b. polls `state.cancellation.active()` every 100ms; if it hits
//!    0 inside `grace`, return early (clean shutdown).
//!    c. if `grace` expires with workers still in flight, call
//!    `state.cancellation.signal_all(RunCommand::DaemonShutdown)`
//!    so the watchdog inside each worker can convert to
//!    `Aborted{DaemonShutdown}` (spec §5.4).
//!    d. await `cron.shutdown()` so the scheduler task stops.
//!
//! The second piece is what tests exercise directly — signal delivery
//! itself is platform-specific (`tokio::signal::unix::SignalKind`) and
//! awkward to integration-test reliably.

use crate::app_state::AppState;
use crate::cron::CronHandle;
use paavo_runner::RunCommand;
use std::time::Duration;
use tokio::time::{sleep, Instant};

/// Await SIGTERM (unix) or Ctrl-C (any platform). Returns when the
/// first such signal arrives.
pub async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "drain: failed to install SIGTERM handler; falling back to Ctrl-C only",
                );
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "drain: failed to install SIGINT handler; falling back to SIGTERM only",
                );
                let _ = term.recv().await;
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => tracing::info!("drain: SIGTERM received"),
            _ = int.recv() => tracing::info!("drain: SIGINT received"),
        }
    }
    #[cfg(not(unix))]
    {
        match tokio::signal::ctrl_c().await {
            Ok(()) => tracing::info!("drain: Ctrl-C received"),
            Err(e) => tracing::error!(error = %e, "drain: ctrl_c handler failed"),
        }
    }
}

/// Drain the daemon: stop accepting new work, wait for in-flight
/// workers to finish (up to `grace`), then force-cancel + stop the
/// cron scheduler. The HTTP server's graceful-shutdown future should
/// fire alongside this so axum stops accepting connections at the
/// same time `state.drain` flips.
///
/// Polls every 100ms. Returns early when the cancellation registry
/// empties before `grace` elapses — a clean shutdown is common.
pub async fn drain_with_grace(state: AppState, cron: CronHandle, grace: Duration) {
    state.drain.set_draining();
    tracing::info!(
        grace_s = grace.as_secs(),
        active = state.cancellation.active(),
        "drain: flipped drain flag, waiting for in-flight workers",
    );
    let deadline = Instant::now() + grace;
    loop {
        if state.cancellation.active() == 0 {
            tracing::info!("drain: all workers finished within grace");
            break;
        }
        if Instant::now() >= deadline {
            let remaining = state.cancellation.active();
            tracing::warn!(
                remaining,
                "drain: grace expired, signaling DaemonShutdown to remaining workers",
            );
            state.cancellation.signal_all(RunCommand::DaemonShutdown);
            // Workers may take a moment to receive + act on the signal.
            // We do NOT block further — paavod::main returns from this
            // function and the runtime drops, killing any survivors.
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    if let Err(e) = cron.shutdown().await {
        tracing::error!(error = %e, "drain: cron scheduler shutdown failed");
    } else {
        tracing::info!("drain: cron scheduler stopped");
    }
}
