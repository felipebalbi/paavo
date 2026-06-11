use chrono::Utc;
use paavo_db::{BoardRow, Db, JobRow, NewJob, OutcomeRecord};
use paavo_proto::{
    BoardHealth, BoardSelector, BoardSpec, JobId, JobOutcome, JobSource, JobState, Priority,
    ProbeSelector, TerminalOutcome,
};
use tempfile::tempdir;

fn fresh_db() -> Db {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    std::mem::forget(dir);
    db
}

/// Insert a single board with id `mcxa266-01`. The `job.board_id` column
/// has `REFERENCES board(id)`, so any test that calls
/// `transition_to_building` must first ensure the referenced board exists.
fn insert_default_board(db: &Db) {
    let spec = BoardSpec {
        id: "mcxa266-01".into(),
        kind: "mcxa266".into(),
        probe_selector: ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: "ABC".into(),
        },
        chip_name: "MCXA266VFL".into(),
        target_name: "frdm-mcx-a266".into(),
        wiring_profile: Some("default".into()),
        health: BoardHealth::Healthy,
    };
    BoardRow::insert(db.raw_conn(), &spec, 0).unwrap();
}

fn sample_new_job(id: JobId) -> NewJob {
    NewJob {
        id,
        priority: Priority::Interactive,
        submitter: "felipe".into(),
        source: JobSource::Cli,
        board_selector: BoardSelector {
            kind: "mcxa266".into(),
            instance: None,
            wiring_profile: None,
        },
        inactivity_timeout_ms: 120_000,
        hard_max_ms: 900_000,
        tar_blake3: "deadbeef".into(),
        tar_path: "/var/lib/paavo/uploads/deadbeef.tar".into(),
        cargo_update_packages: vec![],
    }
}

#[test]
fn insert_then_get_round_trips() {
    let db = fresh_db();
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), now).unwrap();
    let row = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.id, id);
    assert_eq!(row.state, JobState::Submitted);
    assert_eq!(row.priority, Priority::Interactive);
    assert_eq!(row.submitted_at, now);
    assert!(row.outcome.is_none());
    assert!(row.board_id.is_none());
    assert!(row.elf_path.is_none());
}

#[test]
fn next_submitted_returns_highest_priority_oldest_first() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();

    let scheduled = JobId::new();
    let mut sched = sample_new_job(scheduled);
    sched.priority = Priority::Scheduled;
    sched.source = JobSource::Scheduler;
    JobRow::insert(db.raw_conn(), &sched, now).unwrap();

    // Interactive comes 1 ms later than scheduled but should sort first by
    // priority.
    let interactive = JobId::new();
    JobRow::insert(db.raw_conn(), &sample_new_job(interactive), now + 1).unwrap();

    let picks = JobRow::list_submitted(db.raw_conn(), 10).unwrap();
    assert_eq!(picks.len(), 2);
    assert_eq!(picks[0].id, interactive);
    assert_eq!(picks[1].id, scheduled);
}

#[test]
fn transition_to_building_sets_state_and_board_id() {
    let db = fresh_db();
    insert_default_board(&db);
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), now).unwrap();
    JobRow::transition_to_building(db.raw_conn(), &id, "mcxa266-01", now + 10).unwrap();

    let row = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.state, JobState::Building);
    assert_eq!(row.board_id.as_deref(), Some("mcxa266-01"));
    assert_eq!(row.started_at, Some(now + 10));
}

#[test]
fn transition_to_running_records_elf_path() {
    let db = fresh_db();
    insert_default_board(&db);
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), now).unwrap();
    JobRow::transition_to_building(db.raw_conn(), &id, "mcxa266-01", now + 10).unwrap();
    JobRow::transition_to_running(db.raw_conn(), &id, "/cache/abc/foo.elf").unwrap();

    let row = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.state, JobState::Running);
    assert_eq!(row.elf_path.as_deref(), Some("/cache/abc/foo.elf"));
}

#[test]
fn finalize_to_passed_stores_outcome_json() {
    let db = fresh_db();
    insert_default_board(&db);
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), now).unwrap();
    JobRow::transition_to_building(db.raw_conn(), &id, "mcxa266-01", now + 10).unwrap();
    JobRow::transition_to_running(db.raw_conn(), &id, "/cache/foo.elf").unwrap();

    let rec = OutcomeRecord {
        state: JobState::Passed,
        outcome: JobOutcome::Passed,
        finished_at_ms: now + 5_000,
    };
    JobRow::finalize(db.raw_conn(), &id, &rec).unwrap();
    let row = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.state, JobState::Passed);
    assert_eq!(row.outcome, Some(JobOutcome::Passed));
    assert_eq!(row.finished_at, Some(now + 5_000));
}

#[test]
fn finalize_to_failed_with_test_err_round_trips_outcome_detail() {
    let db = fresh_db();
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), now).unwrap();

    let outcome = JobOutcome::Failed(TerminalOutcome::TestErr {
        message: "panicked at 'assertion failed'".into(),
    });
    let rec = OutcomeRecord {
        state: JobState::Failed,
        outcome: outcome.clone(),
        finished_at_ms: now + 2_000,
    };
    JobRow::finalize(db.raw_conn(), &id, &rec).unwrap();

    let row = JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.outcome, Some(outcome));
}

#[test]
fn list_by_state_filters_correctly() {
    let db = fresh_db();
    insert_default_board(&db);
    let now = Utc::now().timestamp_millis();
    let a = JobId::new();
    let b = JobId::new();
    JobRow::insert(db.raw_conn(), &sample_new_job(a), now).unwrap();
    JobRow::insert(db.raw_conn(), &sample_new_job(b), now + 1).unwrap();

    JobRow::transition_to_building(db.raw_conn(), &a, "mcxa266-01", now + 5).unwrap();

    let submitted = JobRow::list_by_state(db.raw_conn(), JobState::Submitted, 50).unwrap();
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].id, b);

    let building = JobRow::list_by_state(db.raw_conn(), JobState::Building, 50).unwrap();
    assert_eq!(building.len(), 1);
    assert_eq!(building[0].id, a);
}

#[test]
fn delete_cascades_to_log_frames() {
    let db = fresh_db();
    let id = JobId::new();
    let now = Utc::now().timestamp_millis();
    JobRow::insert(db.raw_conn(), &sample_new_job(id), now).unwrap();
    db.raw_conn()
        .execute(
            "INSERT INTO log_frame (job_id, seq, ts_us, level, target, message)
             VALUES (?1, 0, 0, 'info', NULL, 'hi')",
            rusqlite::params![id.to_string()],
        )
        .unwrap();

    JobRow::delete(db.raw_conn(), &id).unwrap();
    let count: i64 = db
        .raw_conn()
        .query_row(
            "SELECT COUNT(*) FROM log_frame WHERE job_id = ?1",
            rusqlite::params![id.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn list_recent_returns_jobs_newest_first_across_all_states() {
    let db = fresh_db();
    insert_default_board(&db);
    let now = Utc::now().timestamp_millis();

    // Three jobs spanning Submitted, Building, and Passed — list_recent
    // must include all three regardless of state, newest first.
    let a = JobId::new();
    let b = JobId::new();
    let c = JobId::new();
    JobRow::insert(db.raw_conn(), &sample_new_job(a), now).unwrap();
    JobRow::insert(db.raw_conn(), &sample_new_job(b), now + 10).unwrap();
    JobRow::insert(db.raw_conn(), &sample_new_job(c), now + 20).unwrap();

    JobRow::transition_to_building(db.raw_conn(), &b, "mcxa266-01", now + 11).unwrap();
    JobRow::transition_to_running(db.raw_conn(), &b, "/cache/b.elf").unwrap();
    JobRow::finalize(
        db.raw_conn(),
        &b,
        &OutcomeRecord {
            state: JobState::Passed,
            outcome: JobOutcome::Passed,
            finished_at_ms: now + 12,
        },
    )
    .unwrap();

    let recent = JobRow::list_recent(db.raw_conn(), 10).unwrap();
    assert_eq!(recent.len(), 3);
    assert_eq!(recent[0].id, c, "newest first");
    assert_eq!(recent[1].id, b);
    assert_eq!(recent[2].id, a, "oldest last");
}

#[test]
fn list_recent_respects_limit() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    for i in 0..5 {
        JobRow::insert(db.raw_conn(), &sample_new_job(JobId::new()), now + i).unwrap();
    }
    let recent = JobRow::list_recent(db.raw_conn(), 2).unwrap();
    assert_eq!(recent.len(), 2, "limit clamps the result count");
}

#[test]
fn get_unknown_id_returns_not_found() {
    // Pins that JobRow::get maps QueryReturnedNoRows → DbError::NotFound
    // so the HTTP layer can do `match err { NotFound => 404 }` without
    // pattern-matching on rusqlite primitives.
    let db = fresh_db();
    let ghost = paavo_proto::JobId::new();
    let err = JobRow::get(db.raw_conn(), &ghost).unwrap_err();
    match err {
        paavo_db::DbError::NotFound { entity, id } => {
            assert_eq!(entity, "job");
            assert_eq!(id, ghost.to_string());
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}
