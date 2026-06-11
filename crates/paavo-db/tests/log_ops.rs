use chrono::Utc;
use paavo_db::{Db, JobRow, LogFrameDb, LogFrameRow, NewJob, OutcomeRecord};
use paavo_proto::{
    BoardSelector, JobId, JobOutcome, JobSource, JobState, LogFrame, LogLevel, Priority,
    TerminalOutcome,
};
use tempfile::tempdir;

fn fresh_db() -> Db {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    std::mem::forget(dir);
    db
}

fn enqueue_job(db: &Db) -> JobId {
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(
        db.raw_conn(),
        &NewJob {
            id,
            priority: Priority::Interactive,
            submitter: "test".into(),
            source: JobSource::Cli,
            board_selector: BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
        },
        now,
    )
    .unwrap();
    id
}

fn finalize_passed(db: &Db, id: &JobId) {
    let now = Utc::now().timestamp_millis();
    JobRow::finalize(
        db.raw_conn(),
        id,
        &OutcomeRecord {
            state: JobState::Passed,
            outcome: JobOutcome::Passed,
            finished_at_ms: now,
        },
    )
    .unwrap();
}

#[test]
fn append_then_list_round_trips() {
    let db = fresh_db();
    let id = enqueue_job(&db);

    let frames = vec![
        LogFrame {
            seq: 0,
            ts_us: 100,
            level: LogLevel::Info,
            target: None,
            message: "a".into(),
        },
        LogFrame {
            seq: 1,
            ts_us: 200,
            level: LogLevel::Warn,
            target: Some("foo".into()),
            message: "b".into(),
        },
        LogFrame {
            seq: 2,
            ts_us: 300,
            level: LogLevel::Error,
            target: None,
            message: "c".into(),
        },
    ];
    LogFrameRow::append_batch(db.raw_conn(), &id, &frames).unwrap();

    let got = LogFrameRow::list(db.raw_conn(), &id, 0, 10).unwrap();
    assert_eq!(got, frames);
}

#[test]
fn list_paginates() {
    let db = fresh_db();
    let id = enqueue_job(&db);
    let frames: Vec<_> = (0..50)
        .map(|i| LogFrame {
            seq: i,
            ts_us: i * 100,
            level: LogLevel::Info,
            target: None,
            message: format!("msg-{i}"),
        })
        .collect();
    LogFrameRow::append_batch(db.raw_conn(), &id, &frames).unwrap();

    let page = LogFrameRow::list(db.raw_conn(), &id, 20, 10).unwrap();
    assert_eq!(page.len(), 10);
    assert_eq!(page[0].seq, 20);
    assert_eq!(page[9].seq, 29);
}

#[test]
fn count_for_job_returns_total() {
    let db = fresh_db();
    let id = enqueue_job(&db);
    let frames: Vec<_> = (0..7)
        .map(|i| LogFrame {
            seq: i,
            ts_us: i,
            level: LogLevel::Info,
            target: None,
            message: "x".into(),
        })
        .collect();
    LogFrameRow::append_batch(db.raw_conn(), &id, &frames).unwrap();
    assert_eq!(LogFrameRow::count_for_job(db.raw_conn(), &id).unwrap(), 7);
}

#[test]
fn duplicate_seq_is_rejected() {
    let db = fresh_db();
    let id = enqueue_job(&db);
    let f = LogFrame {
        seq: 0,
        ts_us: 0,
        level: LogLevel::Info,
        target: None,
        message: "x".into(),
    };
    LogFrameRow::append_batch(db.raw_conn(), &id, std::slice::from_ref(&f)).unwrap();
    let err = LogFrameRow::append_batch(db.raw_conn(), &id, std::slice::from_ref(&f)).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("UNIQUE") || msg.contains("PRIMARY KEY"),
        "{msg}"
    );
}

#[test]
fn truncate_passed_keeps_warn_and_error_only() {
    let db = fresh_db();
    let id = enqueue_job(&db);
    let frames = vec![
        LogFrame {
            seq: 0,
            ts_us: 1,
            level: LogLevel::Trace,
            target: None,
            message: "t".into(),
        },
        LogFrame {
            seq: 1,
            ts_us: 2,
            level: LogLevel::Info,
            target: None,
            message: "i".into(),
        },
        LogFrame {
            seq: 2,
            ts_us: 3,
            level: LogLevel::Warn,
            target: None,
            message: "w".into(),
        },
        LogFrame {
            seq: 3,
            ts_us: 4,
            level: LogLevel::Error,
            target: None,
            message: "e".into(),
        },
    ];
    LogFrameRow::append_batch(db.raw_conn(), &id, &frames).unwrap();
    finalize_passed(&db, &id);

    // Pretend "now" is way past the retention horizon.
    let now_ms = Utc::now().timestamp_millis() + 60 * 86_400_000;
    let cut = LogFrameRow::truncate_old_passed(db.raw_conn(), 30, now_ms).unwrap();
    assert_eq!(cut, 2);
    let remaining = LogFrameRow::list(db.raw_conn(), &id, 0, 10).unwrap();
    assert_eq!(remaining.len(), 2);
    assert!(remaining
        .iter()
        .all(|f| matches!(f.level, LogLevel::Warn | LogLevel::Error)));
}

#[test]
fn truncate_disabled_when_days_is_negative() {
    let db = fresh_db();
    let id = enqueue_job(&db);
    let frames = vec![LogFrame {
        seq: 0,
        ts_us: 1,
        level: LogLevel::Trace,
        target: None,
        message: "t".into(),
    }];
    LogFrameRow::append_batch(db.raw_conn(), &id, &frames).unwrap();
    finalize_passed(&db, &id);

    let now_ms = Utc::now().timestamp_millis() + 1_000 * 86_400_000;
    let cut = LogFrameRow::truncate_old_passed(db.raw_conn(), -1, now_ms).unwrap();
    assert_eq!(cut, 0);
    let remaining = LogFrameRow::list(db.raw_conn(), &id, 0, 10).unwrap();
    assert_eq!(remaining.len(), 1);
}

#[test]
fn append_empty_batch_is_noop() {
    let db = fresh_db();
    let id = enqueue_job(&db);
    LogFrame::append_batch(db.raw_conn(), &id, &[]).unwrap();
    // Empty batch should commit cleanly without inserting anything.
    assert_eq!(LogFrame::count_for_job(db.raw_conn(), &id).unwrap(), 0);
}

#[test]
fn count_for_job_returns_zero_for_empty_job() {
    let db = fresh_db();
    let id = enqueue_job(&db);
    // Job exists, but no log_frame rows inserted.
    assert_eq!(LogFrame::count_for_job(db.raw_conn(), &id).unwrap(), 0);
}

#[test]
fn truncate_does_not_touch_non_passed_jobs() {
    let db = fresh_db();
    let id = enqueue_job(&db);

    // Insert a Trace frame (would be in the deletion target if state were passed).
    let frame = LogFrame {
        seq: 0,
        ts_us: 1,
        level: LogLevel::Trace,
        target: None,
        message: "trace".into(),
    };
    LogFrame::append_batch(db.raw_conn(), &id, std::slice::from_ref(&frame)).unwrap();

    // Finalize the job as Failed(TestErr) instead of Passed.
    let now = Utc::now().timestamp_millis();
    JobRow::finalize(
        db.raw_conn(),
        &id,
        &OutcomeRecord {
            state: JobState::Failed,
            outcome: JobOutcome::Failed(TerminalOutcome::TestErr {
                message: "boom".into(),
            }),
            finished_at_ms: now,
        },
    )
    .unwrap();

    // Pretend "now" is way past the retention horizon.
    let way_later = now + 60 * 86_400_000;
    let cut = LogFrame::truncate_old_passed(db.raw_conn(), 30, way_later).unwrap();
    assert_eq!(cut, 0, "non-Passed jobs must not lose frames");

    // The Trace frame should still be present.
    let remaining = LogFrame::list(db.raw_conn(), &id, 0, 10).unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(matches!(remaining[0].level, LogLevel::Trace));
}
