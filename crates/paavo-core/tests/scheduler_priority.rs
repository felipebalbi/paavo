mod common;

use common::{enqueue_with, fresh_db, insert_board};
use paavo_core::{pick_next, SchedulerConfig};
use paavo_proto::{BoardHealth, JobSource, Priority};

const T0: i64 = 1_700_000_000_000;
const NOW: i64 = T0 + 60_000;

#[test]
fn picks_interactive_over_scheduled_even_if_older() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    // Scheduled inserted "first" (submitted_at lower).
    let scheduled = enqueue_with(&db, T0, |req| {
        req.priority = Priority::Scheduled;
        req.source = JobSource::Scheduler;
    });
    let interactive = enqueue_with(&db, T0 + 2_000, |req| {
        req.priority = Priority::Interactive;
        req.source = JobSource::Cli;
    });

    let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW)
        .unwrap()
        .unwrap();
    assert_eq!(pick.job.id, interactive);
    assert_ne!(pick.job.id, scheduled);
}

#[test]
fn returns_none_when_no_healthy_board_matches() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Quarantined);
    enqueue_with(&db, T0, |req| {
        req.priority = Priority::Interactive;
        req.source = JobSource::Cli;
    });

    let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW).unwrap();
    assert!(pick.is_none());
}

#[test]
fn returns_none_when_no_submitted_jobs() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW).unwrap();
    assert!(pick.is_none());
}

#[test]
fn within_a_priority_class_oldest_submitted_wins() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    let a = enqueue_with(&db, T0, |req| {
        req.priority = Priority::Interactive;
        req.source = JobSource::Cli;
    });
    let _b = enqueue_with(&db, T0 + 2_000, |req| {
        req.priority = Priority::Interactive;
        req.source = JobSource::Cli;
    });

    let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW)
        .unwrap()
        .unwrap();
    assert_eq!(pick.job.id, a);
}
