mod common;

use common::{fake_session, into_box};
use crossbeam_channel::unbounded;
use paavo_proto::{JobId, JobOutcome, LogLevel, TimeoutReason};
use paavo_runner::{run_job, JobInputs, JobOutputs};
use std::thread;
use std::time::Duration;

#[test]
fn hard_max_fires_even_when_frames_keep_arriving() {
    let (sess, script) = fake_session();
    let (cancel_tx, cancel_rx) = unbounded();
    let (log_tx, _log_rx) = unbounded();

    let handle = run_job(
        JobInputs {
            job_id: JobId::new(),
            inactivity_timeout_ms: 30_000, // never trips
            hard_max_ms: 400,              // ~half a second
            probe_release_grace_ms: 1_000,
            cancel_rx,
        },
        JobOutputs { log_tx },
        move || into_box(sess),
    );

    // Push a frame every 50 ms for up to 2 s so inactivity can't fire.
    let _producer = thread::spawn(move || {
        for i in 0..40 {
            script.log(LogLevel::Info, &format!("tick {i}"));
            thread::sleep(Duration::from_millis(50));
        }
    });

    let outcome = handle.join();
    drop(cancel_tx);

    match outcome {
        JobOutcome::TimedOut {
            reason: TimeoutReason::HardMax,
            elapsed_ms,
        } => {
            assert!(elapsed_ms >= 400, "elapsed_ms {elapsed_ms} >= 400");
            assert!(elapsed_ms < 3_000, "elapsed_ms {elapsed_ms} < 3000");
        }
        other => panic!("expected TimedOut(HardMax), got {other:?}"),
    }
}
