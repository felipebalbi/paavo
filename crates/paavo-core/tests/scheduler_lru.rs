mod common;

use common::{enqueue_with, fresh_db, insert_board};
use paavo_core::{pick_next, SchedulerConfig};
use paavo_proto::BoardHealth;

const NOW: i64 = 1_700_000_060_000;

#[test]
fn never_used_board_wins_over_recently_used() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    insert_board(&db, "mcxa266-02", "mcxa266", BoardHealth::Healthy);

    // Mark -01 as recently used.
    paavo_db::BoardRow::touch_last_used(db.raw_conn(), "mcxa266-01", NOW - 1_000).unwrap();
    enqueue_with(&db, NOW - 60_000, |_| {});

    let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW)
        .unwrap()
        .unwrap();
    assert_eq!(pick.board.spec.id, "mcxa266-02");
}

#[test]
fn older_last_used_wins_when_both_have_used() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    insert_board(&db, "mcxa266-02", "mcxa266", BoardHealth::Healthy);
    paavo_db::BoardRow::touch_last_used(db.raw_conn(), "mcxa266-01", 500).unwrap();
    paavo_db::BoardRow::touch_last_used(db.raw_conn(), "mcxa266-02", 100).unwrap();
    enqueue_with(&db, NOW - 60_000, |_| {});

    let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW)
        .unwrap()
        .unwrap();
    assert_eq!(pick.board.spec.id, "mcxa266-02");
}

#[test]
fn lru_rotates_across_consecutive_picks() {
    // Realistic dispatch loop: enqueue 3 jobs, drain by repeatedly calling
    // pick_next + touch_last_used + transition_to_building. The boards must
    // rotate in LRU order (never-used first, then ASC by last_used_at).
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    insert_board(&db, "mcxa266-02", "mcxa266", BoardHealth::Healthy);
    insert_board(&db, "mcxa266-03", "mcxa266", BoardHealth::Healthy);

    // Three jobs, distinct submitted_at so the dispatch order within the
    // priority class is deterministic.
    for i in 0..3 {
        enqueue_with(&db, NOW - 60_000 + i, |_| {});
    }

    // Drain 3 picks. Each pick should land on a different board because the
    // previous pick's touch_last_used updates the LRU ordering.
    let mut picked_boards: Vec<String> = Vec::new();
    for tick in 0..3 {
        let pick = pick_next(db.raw_conn(), SchedulerConfig::default(), NOW + tick)
            .unwrap()
            .expect("pick_next returned None mid-drain");
        // Simulate the dispatch loop: mark the job as building and touch the
        // board's last_used_at so the next pick sees fresh LRU state.
        paavo_db::JobRow::transition_to_building(
            db.raw_conn(),
            &pick.job.id,
            &pick.board.spec.id,
            NOW + tick,
        )
        .unwrap();
        paavo_db::BoardRow::touch_last_used(db.raw_conn(), &pick.board.spec.id, NOW + tick)
            .unwrap();
        picked_boards.push(pick.board.spec.id.clone());
    }

    // All three boards used, no repeats — proves LRU rotation works across
    // consecutive picks. (Order is deterministic by the id tiebreaker.)
    let mut sorted = picked_boards.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        3,
        "expected each of the 3 boards to be picked exactly once; got {:?}",
        picked_boards
    );
}
