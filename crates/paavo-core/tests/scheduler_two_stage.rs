mod common;
use common::{enqueue_with, fresh_db, insert_board};
use paavo_core::{pick_buildable, pick_runnable, SchedulerConfig};
use paavo_db::JobRow;
use paavo_proto::{BoardHealth, JobState};

const CFG: SchedulerConfig = SchedulerConfig {
    starvation_threshold_ms: 6 * 60 * 60 * 1000,
};

#[test]
fn pick_buildable_returns_submitted_and_skips_in_flight_blake3() {
    let db = fresh_db();
    insert_board(&db, "b1", "mcxa266", BoardHealth::Healthy);
    // Two submitted jobs, SAME tar_blake3 "x" (default helper value).
    let a = enqueue_with(&db, 100, |_| {});
    let _b = enqueue_with(&db, 200, |_| {});
    // Move A to building → its blake3 "x" is now in-flight.
    JobRow::transition_submitted_to_building(db.raw_conn(), &a, 150).unwrap();
    // Single-flight: B shares blake3 "x", so nothing is buildable.
    assert!(pick_buildable(db.raw_conn(), CFG, 1000).unwrap().is_none());

    // A distinct-blake3 job IS buildable.
    let c = enqueue_with(&db, 300, |req| req.tar_blake3 = "y".into());
    assert_eq!(
        pick_buildable(db.raw_conn(), CFG, 1000)
            .unwrap()
            .unwrap()
            .id,
        c
    );
}

#[test]
fn pick_runnable_returns_awaiting_with_free_board_else_none() {
    let db = fresh_db();
    insert_board(&db, "b1", "mcxa266", BoardHealth::Healthy);
    let a = enqueue_with(&db, 100, |_| {});
    JobRow::transition_submitted_to_building(db.raw_conn(), &a, 110).unwrap();
    JobRow::transition_building_to_awaiting_board(db.raw_conn(), &a, "/e.elf").unwrap();

    // Board free → runnable.
    let pick = pick_runnable(db.raw_conn(), CFG, 1000).unwrap().unwrap();
    assert_eq!(pick.job.id, a);
    assert_eq!(pick.board.spec.id, "b1");

    // Occupy the board with a running job → no longer runnable.
    JobRow::transition_awaiting_to_running(db.raw_conn(), &a, "b1").unwrap();
    let b = enqueue_with(&db, 200, |_| {});
    JobRow::transition_submitted_to_building(db.raw_conn(), &b, 210).unwrap();
    JobRow::transition_building_to_awaiting_board(db.raw_conn(), &b, "/e2.elf").unwrap();
    assert_eq!(
        pick_runnable(db.raw_conn(), CFG, 1000)
            .unwrap()
            .map(|p| p.job.id),
        None
    );
    assert_eq!(
        JobState::AwaitingBoard,
        JobRow::get(db.raw_conn(), &b).unwrap().state
    );
}
