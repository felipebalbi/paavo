mod common;

use common::{
    default_enqueue_request, fresh_db, insert_board, list_inventory_specs, DAEMON_CEILING_8H_MS,
    OVER_CEILING_9H_MS,
};
use paavo_core::{enqueue_job, CoreError};
use paavo_proto::BoardHealth;

#[test]
fn enqueue_inserts_a_submitted_job() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    let mut req = default_enqueue_request("mcxa266");
    // The default selector has no wiring_profile; pin it for this test to
    // confirm the wiring_profile=Some path round-trips.
    req.board_selector.wiring_profile = Some("default".into());
    let id = req.job_id;
    let now_ms = chrono::Utc::now().timestamp_millis();

    let returned = enqueue_job(db.raw_conn(), &list_inventory_specs(&db), req, now_ms).unwrap();
    assert_eq!(returned, id);

    let row = paavo_db::JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.state, paavo_proto::JobState::Submitted);
}

#[test]
fn rejects_selector_with_no_matching_board() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    let req = default_enqueue_request("mcxap266"); // typo'd kind
    let err = enqueue_job(
        db.raw_conn(),
        &list_inventory_specs(&db),
        req,
        chrono::Utc::now().timestamp_millis(),
    )
    .unwrap_err();
    // Fix I-2: assert the error reports the exact selector kind we passed.
    assert!(
        matches!(&err, CoreError::SelectorNeverMatches(s) if s.kind == "mcxap266"),
        "{err}"
    );
}

#[test]
fn rejects_quarantined_only_kind_too() {
    // Per spec §5.5 the selector must be *possible*, not currently available.
    // A quarantined board is still possible — bring it back online with
    // unquarantine and it can run. So this case should be accepted.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Quarantined);

    let req = default_enqueue_request("mcxa266");
    let id = req.job_id;
    let returned = enqueue_job(
        db.raw_conn(),
        &list_inventory_specs(&db),
        req,
        chrono::Utc::now().timestamp_millis(),
    )
    .unwrap();
    assert_eq!(returned, id);

    // Fix I-1: verify the row actually landed in `job`, not just that
    // enqueue_job returned Ok.
    let row = paavo_db::JobRow::get(db.raw_conn(), &id).unwrap();
    assert_eq!(row.state, paavo_proto::JobState::Submitted);
}

#[test]
fn rejects_hard_max_above_daemon_ceiling() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    let mut req = default_enqueue_request("mcxa266");
    req.hard_max_ms = OVER_CEILING_9H_MS;
    let err = enqueue_job(
        db.raw_conn(),
        &list_inventory_specs(&db),
        req,
        chrono::Utc::now().timestamp_millis(),
    )
    .unwrap_err();
    assert!(
        matches!(err, CoreError::OverCeiling { requested, ceiling }
            if requested == OVER_CEILING_9H_MS && ceiling == DAEMON_CEILING_8H_MS),
        "{err}",
    );
}

#[test]
fn rejects_selector_with_mismatched_instance() {
    // The kind matches but the instance pins a board id that doesn't exist.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    let mut req = default_enqueue_request("mcxa266");
    req.board_selector.instance = Some("mcxa266-99".into());
    let err = enqueue_job(
        db.raw_conn(),
        &list_inventory_specs(&db),
        req,
        chrono::Utc::now().timestamp_millis(),
    )
    .unwrap_err();
    assert!(
        matches!(&err, CoreError::SelectorNeverMatches(s)
            if s.kind == "mcxa266"
                && s.instance.as_deref() == Some("mcxa266-99")),
        "{err}"
    );
}

#[test]
fn rejects_selector_with_mismatched_wiring_profile() {
    // The kind matches but the wiring_profile pins a profile that no
    // registered board offers.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    // The default insert_board() uses wiring_profile=Some("default").

    let mut req = default_enqueue_request("mcxa266");
    req.board_selector.wiring_profile = Some("never-existed".into());
    let err = enqueue_job(
        db.raw_conn(),
        &list_inventory_specs(&db),
        req,
        chrono::Utc::now().timestamp_millis(),
    )
    .unwrap_err();
    assert!(
        matches!(&err, CoreError::SelectorNeverMatches(s)
            if s.wiring_profile.as_deref() == Some("never-existed")),
        "{err}"
    );
}
