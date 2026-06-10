mod common;

use common::{fake_session, into_box};
use crossbeam_channel::unbounded;
use paavo_proto::{JobId, JobOutcome, TimeoutReason};
use paavo_runner::{run_job, JobInputs, JobOutputs};

#[test]
fn inactivity_timeout_fires_when_no_events_arrive() {
    let (sess, _script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 200,
            hard_max_ms: 30_000,
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    let outcome = handle.join();
    match outcome {
        JobOutcome::TimedOut {
            reason: TimeoutReason::Inactivity,
            elapsed_ms,
        } => {
            assert!(
                elapsed_ms >= 200,
                "elapsed_ms {elapsed_ms} should be >= 200"
            );
            assert!(
                elapsed_ms < 2_000,
                "elapsed_ms {elapsed_ms} should be < 2000"
            );
        }
        other => panic!("expected TimedOut(Inactivity), got {other:?}"),
    }
    drop(cancel_tx);
}
