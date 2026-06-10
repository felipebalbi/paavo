mod common;

use common::{fake_session, into_box};
use crossbeam_channel::unbounded;
use paavo_proto::{AbortReason, JobId, JobOutcome};
use paavo_runner::{run_job, JobInputs, JobOutputs, RunCommand};
use std::thread;
use std::time::Duration;

#[test]
fn user_cancel_produces_aborted_user() {
    let (sess, _script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 30_000,
            hard_max_ms: 30_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    thread::sleep(Duration::from_millis(150));
    cancel_tx.send(RunCommand::Cancel).unwrap();

    let outcome = handle.join();
    assert_eq!(
        outcome,
        JobOutcome::Aborted {
            by: AbortReason::User
        }
    );
}

#[test]
fn daemon_shutdown_produces_aborted_daemon_shutdown() {
    let (sess, _script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 30_000,
            hard_max_ms: 30_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    thread::sleep(Duration::from_millis(150));
    cancel_tx.send(RunCommand::DaemonShutdown).unwrap();

    let outcome = handle.join();
    assert_eq!(
        outcome,
        JobOutcome::Aborted {
            by: AbortReason::DaemonShutdown
        }
    );
}
