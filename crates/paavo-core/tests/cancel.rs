mod common;

use common::{enqueue_with, fresh_db, insert_board};
use paavo_core::CoreError;
use paavo_proto::{AbortReason, BoardHealth, JobOutcome, JobState};

const NOW: i64 = 1_700_000_000_000;

#[test]
fn cancel_submitted_job_finalizes_with_aborted_user() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let id = enqueue_with(&db, NOW, |_| {});

    let outcome = paavo_core::cancel_if_submitted(db.raw_conn(), &id, NOW + 1).unwrap();
    assert_eq!(
        outcome,
        Some(JobOutcome::Aborted {
            by: AbortReason::User
        })
    );
    let row = paavo_db::JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.state, JobState::Aborted);
    assert_eq!(
        row.outcome,
        Some(JobOutcome::Aborted {
            by: AbortReason::User
        })
    );
}

#[test]
fn cancel_running_job_returns_not_cancellable_inline() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let id = enqueue_with(&db, NOW, |_| {});

    // Force into Running state.
    paavo_db::JobRow::transition_to_building(db.raw_conn(), &id, "mcxa266-01", NOW + 1).unwrap();
    paavo_db::JobRow::transition_to_running(db.raw_conn(), &id, "/cache/foo.elf").unwrap();

    let res = paavo_core::cancel_if_submitted(db.raw_conn(), &id, NOW + 2);
    let err = res.unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::NotCancellable {
                state: JobState::Running
            }
        ),
        "{err}"
    );
}

#[test]
fn cancel_already_finalized_returns_not_cancellable() {
    // Aborted/Passed/Failed/TimedOut are all terminal; cancel must reject.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let id = enqueue_with(&db, NOW, |_| {});

    // First cancel succeeds.
    paavo_core::cancel_if_submitted(db.raw_conn(), &id, NOW + 1).unwrap();

    // Second cancel must reject; state is now Aborted.
    let err = paavo_core::cancel_if_submitted(db.raw_conn(), &id, NOW + 2).unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::NotCancellable {
                state: JobState::Aborted
            }
        ),
        "{err}"
    );
}

#[test]
fn cancel_building_job_returns_not_cancellable() {
    // Submitted is the only state cancel_if_submitted handles inline.
    // Building (post-build, pre-flash) must surface NotCancellable too,
    // not silently fall through to the Submitted branch.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let id = enqueue_with(&db, NOW, |_| {});

    paavo_db::JobRow::transition_to_building(db.raw_conn(), &id, "mcxa266-01", NOW + 1).unwrap();
    // Note: NOT transitioning to Running.

    let res = paavo_core::cancel_if_submitted(db.raw_conn(), &id, NOW + 2);
    let err = res.unwrap_err();
    assert!(
        matches!(
            err,
            CoreError::NotCancellable {
                state: JobState::Building
            }
        ),
        "{err}"
    );
}
