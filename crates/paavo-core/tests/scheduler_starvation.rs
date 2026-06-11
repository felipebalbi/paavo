mod common;

use common::{enqueue_with, fresh_db, insert_board};
use paavo_core::{pick_next, SchedulerConfig};
use paavo_proto::{BoardHealth, JobSource, Priority};

#[test]
fn scheduled_job_older_than_threshold_outranks_a_fresh_interactive() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    // Use injected timestamps so the test is race-free.
    const T_SCHEDULED: i64 = 1_700_000_000_000;
    const T_INTERACTIVE: i64 = T_SCHEDULED + 80; // ms
    const NOW: i64 = T_INTERACTIVE + 1;
    let cfg = SchedulerConfig {
        starvation_threshold_ms: 50,
    };

    let scheduled = enqueue_with(&db, T_SCHEDULED, |req| {
        req.priority = Priority::Scheduled;
        req.source = JobSource::Scheduler;
        req.submitter = "scheduler".into();
    });
    let _interactive = enqueue_with(&db, T_INTERACTIVE, |req| {
        req.priority = Priority::Interactive;
        req.source = JobSource::Cli;
        req.submitter = "cli".into();
    });

    let pick = pick_next(db.raw_conn(), cfg, NOW).unwrap().unwrap();
    assert_eq!(
        pick.job.id, scheduled,
        "starved Scheduled job should outrank fresh Interactive"
    );
}

#[test]
fn scheduled_job_within_threshold_does_not_promote() {
    // Mirror image of the above: when now-submitted_at < threshold, the
    // Scheduled job is NOT promoted and the fresh Interactive wins.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    const T_SCHEDULED: i64 = 1_700_000_000_000;
    const T_INTERACTIVE: i64 = T_SCHEDULED + 30; // ms
    const NOW: i64 = T_INTERACTIVE + 1;
    let cfg = SchedulerConfig {
        starvation_threshold_ms: 50, // 50ms threshold, scheduled is only 31ms old at NOW
    };

    let _scheduled = enqueue_with(&db, T_SCHEDULED, |req| {
        req.priority = Priority::Scheduled;
        req.source = JobSource::Scheduler;
    });
    let interactive = enqueue_with(&db, T_INTERACTIVE, |req| {
        req.priority = Priority::Interactive;
        req.source = JobSource::Cli;
    });

    let pick = pick_next(db.raw_conn(), cfg, NOW).unwrap().unwrap();
    assert_eq!(
        pick.job.id, interactive,
        "non-starved Scheduled job should not outrank a fresh Interactive"
    );
}

#[test]
fn scheduled_job_at_exactly_threshold_promotes() {
    // Spec rule: starvation promotion fires when now - submitted_at >= threshold.
    // This locks down the `>=` (not `>`) boundary so a refactor that flips the
    // comparison would fail loudly.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    const T_SCHEDULED: i64 = 1_700_000_000_000;
    const NOW: i64 = T_SCHEDULED + 50; // exactly at threshold
    let cfg = SchedulerConfig {
        starvation_threshold_ms: 50,
    };

    let scheduled = enqueue_with(&db, T_SCHEDULED, |req| {
        req.priority = Priority::Scheduled;
        req.source = JobSource::Scheduler;
    });
    // Interactive submitted "now" so it would normally win.
    let _interactive = enqueue_with(&db, NOW, |req| {
        req.priority = Priority::Interactive;
        req.source = JobSource::Cli;
    });

    let pick = pick_next(db.raw_conn(), cfg, NOW).unwrap().unwrap();
    assert_eq!(
        pick.job.id, scheduled,
        "at exactly the threshold, scheduled must promote (>= boundary)"
    );
}
