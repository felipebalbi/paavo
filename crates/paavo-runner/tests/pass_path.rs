mod common;

use common::{fake_session, into_box};
use crossbeam_channel::unbounded;
use paavo_proto::{JobId, JobOutcome, LogLevel};
use paavo_runner::{run_job, JobInputs, JobOutputs};

#[test]
fn test_ok_then_bkpt_produces_passed() {
    let (sess, script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 60_000,
            hard_max_ms: 60_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    script.log(LogLevel::Info, "boot complete");
    script.log(LogLevel::Info, "Test OK");
    script.bkpt();

    let outcome = handle.join();
    assert_eq!(outcome, JobOutcome::Passed);
    drop(cancel_tx);

    // Two frames should have been forwarded; the bkpt is consumed silently.
    let frames: Vec<_> = log_rx.try_iter().collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1].message, "Test OK");
}

#[test]
fn panic_event_produces_failed_testerr() {
    let (sess, script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 60_000,
            hard_max_ms: 60_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    script.panic("assertion failed: x == 0");
    let outcome = handle.join();
    match outcome {
        JobOutcome::Failed(paavo_proto::TerminalOutcome::TestErr { message }) => {
            assert!(message.contains("assertion failed"), "{message}");
        }
        other => panic!("expected Failed(TestErr), got {other:?}"),
    }
    drop(cancel_tx);
}

#[test]
fn disconnect_event_produces_failed_infraerr() {
    let (sess, script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 60_000,
            hard_max_ms: 60_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    script.disconnect();
    let outcome = handle.join();
    match outcome {
        JobOutcome::Failed(paavo_proto::TerminalOutcome::InfraErr { stage, .. }) => {
            assert_eq!(stage, "probe_disconnect");
        }
        other => panic!("expected Failed(InfraErr), got {other:?}"),
    }
    drop(cancel_tx);
}

#[test]
fn connect_failure_produces_infra_err() {
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 60_000,
            hard_max_ms: 60_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        common::fail_to_connect,
    );

    let outcome = handle.join();
    match outcome {
        JobOutcome::Failed(paavo_proto::TerminalOutcome::InfraErr { stage, .. }) => {
            assert_eq!(stage, "probe_attach");
        }
        other => panic!("expected Failed(InfraErr probe_attach), got {other:?}"),
    }
    drop(cancel_tx);
}

#[test]
fn bkpt_without_preceding_test_ok_produces_failed_testerr() {
    let (sess, script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 60_000,
            hard_max_ms: 60_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    // Bkpt arrives with no prior "Test OK" marker. Worker must classify
    // this as a test error (not a pass).
    script.bkpt();

    let outcome = handle.join();
    match outcome {
        JobOutcome::Failed(paavo_proto::TerminalOutcome::TestErr { message }) => {
            assert!(
                message.contains("bkpt without preceding Test OK"),
                "expected diagnostic message, got: {message}"
            );
        }
        other => panic!("expected Failed(TestErr 'bkpt without Test OK'), got {other:?}"),
    }
    drop(cancel_tx);
}
