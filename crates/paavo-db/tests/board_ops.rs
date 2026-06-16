use chrono::Utc;
use paavo_db::{BoardRow, Db};
use paavo_proto::{BoardHealth, BoardSpec, ProbeSelector};
use tempfile::tempdir;

fn fresh_db() -> Db {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    std::mem::forget(dir); // tempdir lives for the test process
    db
}

fn sample_board() -> BoardSpec {
    BoardSpec {
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
    }
}

#[test]
fn insert_then_get_round_trips() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();

    let got = BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(got.spec, sample_board());
    assert_eq!(got.consecutive_infra_failures, 0);
    assert_eq!(got.created_at, now);
}

#[test]
fn list_all_returns_inserted_boards_sorted_by_id() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    let mut a = sample_board();
    a.id = "mcxa266-02".into();
    let mut b = sample_board();
    b.id = "mcxa266-01".into();
    BoardRow::insert(db.raw_conn(), &a, now).unwrap();
    BoardRow::insert(db.raw_conn(), &b, now).unwrap();

    let rows = BoardRow::list_all(db.raw_conn()).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].spec.id, "mcxa266-01");
    assert_eq!(rows[1].spec.id, "mcxa266-02");
}

#[test]
fn find_healthy_for_selector_filters_by_kind_and_excludes_quarantined() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();

    let mut healthy_mcx = sample_board();
    healthy_mcx.id = "mcxa266-01".into();
    BoardRow::insert(db.raw_conn(), &healthy_mcx, now).unwrap();

    let mut quarantined_mcx = sample_board();
    quarantined_mcx.id = "mcxa266-02".into();
    quarantined_mcx.health = BoardHealth::Quarantined;
    BoardRow::insert(db.raw_conn(), &quarantined_mcx, now).unwrap();

    let mut healthy_rt = sample_board();
    healthy_rt.id = "rt685-01".into();
    healthy_rt.kind = "rt685-evk".into();
    BoardRow::insert(db.raw_conn(), &healthy_rt, now).unwrap();

    let sel = paavo_proto::BoardSelector {
        kind: "mcxa266".into(),
        instance: None,
        wiring_profile: None,
    };
    let rows = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].spec.id, "mcxa266-01");
}

#[test]
fn touch_last_used_updates_only_that_column() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();
    BoardRow::touch_last_used(db.raw_conn(), "mcxa266-01", now + 1000).unwrap();
    let row = BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.last_used_at, Some(now + 1000));
    assert_eq!(row.created_at, now);
}

#[test]
fn quarantine_and_unquarantine_flip_health_and_reset_counter() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();
    BoardRow::bump_infra_failure(db.raw_conn(), "mcxa266-01").unwrap();
    BoardRow::bump_infra_failure(db.raw_conn(), "mcxa266-01").unwrap();
    let row = BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.consecutive_infra_failures, 2);

    BoardRow::quarantine(db.raw_conn(), "mcxa266-01", "broken header").unwrap();
    let row = BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Quarantined);
    assert_eq!(row.quarantine_reason.as_deref(), Some("broken header"));
    assert_eq!(
        row.consecutive_infra_failures, 2,
        "quarantine() must not reset the infra-failure counter — only unquarantine() does that"
    );

    BoardRow::unquarantine(db.raw_conn(), "mcxa266-01").unwrap();
    let row = BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Healthy);
    assert_eq!(row.consecutive_infra_failures, 0);
    assert!(row.quarantine_reason.is_none());
}

#[test]
fn reset_infra_failures_clears_counter_on_pass() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();
    BoardRow::bump_infra_failure(db.raw_conn(), "mcxa266-01").unwrap();
    BoardRow::reset_infra_failures(db.raw_conn(), "mcxa266-01").unwrap();
    let row = BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.consecutive_infra_failures, 0);
}

#[test]
fn find_healthy_for_selector_filters_by_wiring_profile_only() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();

    let mut a = sample_board();
    a.id = "mcxa266-01".into();
    a.wiring_profile = Some("default".into());
    BoardRow::insert(db.raw_conn(), &a, now).unwrap();

    let mut b = sample_board();
    b.id = "mcxa266-02".into();
    b.wiring_profile = Some("alt-spi".into());
    BoardRow::insert(db.raw_conn(), &b, now).unwrap();

    let sel = paavo_proto::BoardSelector {
        kind: "mcxa266".into(),
        instance: None,
        wiring_profile: Some("alt-spi".into()),
    };
    let rows = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].spec.id, "mcxa266-02");
}

#[test]
fn find_healthy_for_selector_filters_by_instance_only() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();

    let mut a = sample_board();
    a.id = "mcxa266-01".into();
    BoardRow::insert(db.raw_conn(), &a, now).unwrap();

    let mut b = sample_board();
    b.id = "mcxa266-02".into();
    BoardRow::insert(db.raw_conn(), &b, now).unwrap();

    let sel = paavo_proto::BoardSelector {
        kind: "mcxa266".into(),
        instance: Some("mcxa266-02".into()),
        wiring_profile: None,
    };
    let rows = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].spec.id, "mcxa266-02");
}

#[test]
fn find_healthy_for_selector_filters_by_instance_and_wiring_profile() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();

    let mut a = sample_board();
    a.id = "mcxa266-01".into();
    a.wiring_profile = Some("default".into());
    BoardRow::insert(db.raw_conn(), &a, now).unwrap();

    let mut b = sample_board();
    b.id = "mcxa266-02".into();
    b.wiring_profile = Some("alt-spi".into());
    BoardRow::insert(db.raw_conn(), &b, now).unwrap();

    // Both clauses must match.
    let sel = paavo_proto::BoardSelector {
        kind: "mcxa266".into(),
        instance: Some("mcxa266-02".into()),
        wiring_profile: Some("alt-spi".into()),
    };
    let rows = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].spec.id, "mcxa266-02");

    // If either clause doesn't match, zero results.
    let sel2 = paavo_proto::BoardSelector {
        kind: "mcxa266".into(),
        instance: Some("mcxa266-02".into()),
        wiring_profile: Some("default".into()), // wrong profile for -02
    };
    let rows2 = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel2).unwrap();
    assert!(rows2.is_empty());
}

#[test]
fn insert_duplicate_id_returns_already_exists() {
    let db = fresh_db();
    let now = chrono::Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();
    let err = BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap_err();
    match err {
        paavo_db::DbError::AlreadyExists { entity, id } => {
            assert_eq!(entity, "board");
            assert_eq!(id, "mcxa266-01");
        }
        other => panic!("expected AlreadyExists, got {other:?}"),
    }
}

#[test]
fn quarantine_unknown_id_returns_not_found() {
    let db = fresh_db();
    let err = BoardRow::quarantine(db.raw_conn(), "ghost", "reason").unwrap_err();
    match err {
        paavo_db::DbError::NotFound { entity, id } => {
            assert_eq!(entity, "board");
            assert_eq!(id, "ghost");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn unquarantine_unknown_id_returns_not_found() {
    let db = fresh_db();
    let err = BoardRow::unquarantine(db.raw_conn(), "ghost").unwrap_err();
    assert!(matches!(err, paavo_db::DbError::NotFound { .. }));
}

#[test]
fn touch_last_used_unknown_id_returns_not_found() {
    let db = fresh_db();
    let err = BoardRow::touch_last_used(db.raw_conn(), "ghost", 1).unwrap_err();
    assert!(matches!(err, paavo_db::DbError::NotFound { .. }));
}

#[test]
fn bump_infra_failure_unknown_id_returns_not_found() {
    let db = fresh_db();
    let err = BoardRow::bump_infra_failure(db.raw_conn(), "ghost").unwrap_err();
    assert!(matches!(err, paavo_db::DbError::NotFound { .. }));
}

#[test]
fn reset_infra_failures_unknown_id_returns_not_found() {
    let db = fresh_db();
    let err = BoardRow::reset_infra_failures(db.raw_conn(), "ghost").unwrap_err();
    assert!(matches!(err, paavo_db::DbError::NotFound { .. }));
}

#[test]
fn get_unknown_id_returns_not_found() {
    // Pins that BoardRow::get maps QueryReturnedNoRows → DbError::NotFound
    // for consistency with the mutator helpers.
    let db = fresh_db();
    let err = BoardRow::get(db.raw_conn(), "ghost").unwrap_err();
    match err {
        paavo_db::DbError::NotFound { entity, id } => {
            assert_eq!(entity, "board");
            assert_eq!(id, "ghost");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn find_healthy_for_selector_excludes_boards_with_in_flight_jobs() {
    // Hardware-safety invariant: a board currently RUNNING a job must
    // NOT appear in the scheduler's eligible-boards list. Only the run
    // phase holds a board now (build is board-free), so only `running`
    // excludes. Without this guard the run stage could claim the same
    // board for two concurrent jobs and drive two probes at once.
    let db = fresh_db();
    let now = chrono::Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();

    let sel = paavo_proto::BoardSelector {
        kind: "mcxa266".into(),
        instance: None,
        wiring_profile: None,
    };
    // No jobs yet → board is eligible.
    let rows = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel).unwrap();
    assert_eq!(rows.len(), 1, "healthy board should appear when no jobs");

    // Insert a job and walk it through the two-stage lifecycle. Only a
    // `running` job holds a board now, so only that state excludes it.
    let job_id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        db.raw_conn(),
        &paavo_db::NewJob {
            id: job_id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: sel.clone(),
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        now,
    )
    .unwrap();
    // Building holds NO board → board stays eligible.
    paavo_db::JobRow::transition_submitted_to_building(db.raw_conn(), &job_id, now + 1).unwrap();
    let rows = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "a building (board-free) job must not exclude the board"
    );

    // AwaitingBoard still holds no board → still eligible.
    paavo_db::JobRow::transition_building_to_awaiting_board(db.raw_conn(), &job_id, "/elf")
        .unwrap();
    let rows = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "an awaiting_board job must not exclude the board"
    );

    // Running claims the board → now excluded.
    paavo_db::JobRow::transition_awaiting_to_running(db.raw_conn(), &job_id, "mcxa266-01").unwrap();
    let rows = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel).unwrap();
    assert!(rows.is_empty(), "board with a Running job must be excluded");

    // Finalize → board is free again.
    paavo_db::JobRow::finalize(
        db.raw_conn(),
        &job_id,
        &paavo_db::OutcomeRecord {
            state: paavo_proto::JobState::Passed,
            outcome: paavo_proto::JobOutcome::Passed,
            finished_at_ms: now + 2,
        },
    )
    .unwrap();
    let rows = BoardRow::find_healthy_for_selector(db.raw_conn(), &sel).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "board should be eligible again after job finalized"
    );
}

#[test]
fn delete_quarantined_board_succeeds() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();
    BoardRow::quarantine(db.raw_conn(), "mcxa266-01", "broken").unwrap();

    BoardRow::delete(db.raw_conn(), "mcxa266-01").unwrap();

    assert!(
        BoardRow::find(db.raw_conn(), "mcxa266-01")
            .unwrap()
            .is_none(),
        "row should be gone after delete"
    );
}

#[test]
fn delete_unknown_id_returns_not_found() {
    let db = fresh_db();
    let err = BoardRow::delete(db.raw_conn(), "ghost").unwrap_err();
    match err {
        paavo_db::DbError::NotFound { entity, id } => {
            assert_eq!(entity, "board");
            assert_eq!(id, "ghost");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn delete_healthy_board_returns_conflict_must_be_quarantined_first() {
    // §9.4: delete is a destructive op and is gated by quarantine so an
    // operator cannot accidentally yank a healthy board out from under
    // running jobs. The "quarantined first" substring is load-bearing
    // for the HTTP layer's 400-vs-409 routing in db_to_http.
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();
    let err = BoardRow::delete(db.raw_conn(), "mcxa266-01").unwrap_err();
    match err {
        paavo_db::DbError::Conflict { entity, id, reason } => {
            assert_eq!(entity, "board");
            assert_eq!(id, "mcxa266-01");
            assert!(
                reason.contains("quarantined first"),
                "reason should mention 'quarantined first', got: {reason}"
            );
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    // And the row is still there.
    assert!(BoardRow::find(db.raw_conn(), "mcxa266-01")
        .unwrap()
        .is_some());
}

#[test]
fn delete_board_with_referencing_job_returns_conflict_via_fk() {
    // SQLite foreign-key enforcement (PRAGMA foreign_keys = ON, set at
    // Db::open) refuses the DELETE because job.board_id references this
    // row. The typed Conflict reason should mention the referencing
    // jobs so the operator knows why and what to do (wait for retention).
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BoardRow::insert(db.raw_conn(), &sample_board(), now).unwrap();

    let sel = paavo_proto::BoardSelector {
        kind: "mcxa266".into(),
        instance: None,
        wiring_profile: None,
    };
    let job_id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        db.raw_conn(),
        &paavo_db::NewJob {
            id: job_id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: sel,
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        now,
    )
    .unwrap();
    // Wire the job to this board via the building transition so
    // job.board_id is populated.
    paavo_db::JobRow::transition_to_building(db.raw_conn(), &job_id, "mcxa266-01", now + 1)
        .unwrap();

    // Even if we now quarantine the board, the FK still refuses delete.
    BoardRow::quarantine(db.raw_conn(), "mcxa266-01", "draining").unwrap();
    let err = BoardRow::delete(db.raw_conn(), "mcxa266-01").unwrap_err();
    match err {
        paavo_db::DbError::Conflict { entity, id, reason } => {
            assert_eq!(entity, "board");
            assert_eq!(id, "mcxa266-01");
            assert!(
                reason.contains("referenced by") && reason.contains("job"),
                "reason should mention referencing job rows, got: {reason}"
            );
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
    // Row still present.
    assert!(BoardRow::find(db.raw_conn(), "mcxa266-01")
        .unwrap()
        .is_some());
}
