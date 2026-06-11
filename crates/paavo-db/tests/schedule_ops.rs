use paavo_db::{Db, ScheduleRow, ScheduleUpdate};
use tempfile::tempdir;

fn fresh_db() -> Db {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    std::mem::forget(dir);
    db
}

#[test]
fn upsert_then_get() {
    let db = fresh_db();
    ScheduleRow::upsert(
        db.raw_conn(),
        &ScheduleRow {
            id: "nightly".into(),
            cron: "0 19 * * *".into(),
            enabled: true,
            last_triggered_at: None,
            last_completed_at: None,
        },
    )
    .unwrap();
    let r = ScheduleRow::get(db.raw_conn(), "nightly").unwrap();
    assert_eq!(r.cron, "0 19 * * *");
    assert!(r.enabled);
    assert!(r.last_triggered_at.is_none());
}

#[test]
fn apply_update_sets_triggered_then_completed() {
    let db = fresh_db();
    ScheduleRow::upsert(
        db.raw_conn(),
        &ScheduleRow {
            id: "nightly".into(),
            cron: "0 19 * * *".into(),
            enabled: true,
            last_triggered_at: None,
            last_completed_at: None,
        },
    )
    .unwrap();
    ScheduleRow::apply_update(
        db.raw_conn(),
        "nightly",
        &ScheduleUpdate {
            last_triggered_at: Some(100),
            last_completed_at: None,
        },
    )
    .unwrap();
    let r = ScheduleRow::get(db.raw_conn(), "nightly").unwrap();
    assert_eq!(r.last_triggered_at, Some(100));

    ScheduleRow::apply_update(
        db.raw_conn(),
        "nightly",
        &ScheduleUpdate {
            last_triggered_at: None,
            last_completed_at: Some(200),
        },
    )
    .unwrap();
    let r = ScheduleRow::get(db.raw_conn(), "nightly").unwrap();
    assert_eq!(r.last_completed_at, Some(200));
    assert_eq!(r.last_triggered_at, Some(100));
}

#[test]
fn second_upsert_preserves_timestamps() {
    let db = fresh_db();

    // First insert: schedule with a baseline cron, no timestamps yet.
    ScheduleRow::upsert(
        db.raw_conn(),
        &ScheduleRow {
            id: "nightly".into(),
            cron: "0 19 * * *".into(),
            enabled: true,
            last_triggered_at: None,
            last_completed_at: None,
        },
    )
    .unwrap();

    // Simulate the cron firing — set both timestamps via apply_update.
    ScheduleRow::apply_update(
        db.raw_conn(),
        "nightly",
        &ScheduleUpdate {
            last_triggered_at: Some(100),
            last_completed_at: Some(200),
        },
    )
    .unwrap();

    // Second upsert: paavod reloads the config with a *different* cron, but
    // passes timestamps it doesn't know about. The ON CONFLICT path must
    // update cron + enabled while preserving the in-DB last_*_at values.
    ScheduleRow::upsert(
        db.raw_conn(),
        &ScheduleRow {
            id: "nightly".into(),
            cron: "30 19 * * *".into(), // changed
            enabled: false,             // changed
            last_triggered_at: None,    // not passed back
            last_completed_at: None,    // not passed back
        },
    )
    .unwrap();

    let after = ScheduleRow::get(db.raw_conn(), "nightly").unwrap();
    assert_eq!(after.cron, "30 19 * * *", "cron should have been updated");
    assert!(!after.enabled, "enabled should have been updated");
    assert_eq!(
        after.last_triggered_at,
        Some(100),
        "last_triggered_at must be preserved on conflict"
    );
    assert_eq!(
        after.last_completed_at,
        Some(200),
        "last_completed_at must be preserved on conflict"
    );
}

#[test]
fn get_on_missing_id_errors() {
    let db = fresh_db();
    let err = ScheduleRow::get(db.raw_conn(), "nonexistent").unwrap_err();
    match err {
        paavo_db::DbError::NotFound { entity, id } => {
            assert_eq!(entity, "schedule");
            assert_eq!(id, "nonexistent");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn upsert_coalesces_last_triggered_at_on_conflict() {
    // Pins the M4.3.c Round 2 fix: a second upsert with a fresh
    // last_triggered_at must update the column, not silently preserve
    // the first-fire value. Without COALESCE in the DO UPDATE clause,
    // the cron driver's "bump triggered on every fire" pattern would
    // be silently no-op'd, and paavo-web's /schedule would show a
    // fossil timestamp.
    let db = fresh_db();
    ScheduleRow::upsert(
        db.raw_conn(),
        &ScheduleRow {
            id: "nightly".into(),
            cron: "0 0 19 * * *".into(),
            enabled: true,
            last_triggered_at: Some(1_000),
            last_completed_at: None,
        },
    )
    .unwrap();
    ScheduleRow::upsert(
        db.raw_conn(),
        &ScheduleRow {
            id: "nightly".into(),
            cron: "0 0 19 * * *".into(),
            enabled: true,
            last_triggered_at: Some(2_000),
            last_completed_at: None,
        },
    )
    .unwrap();
    let row = ScheduleRow::get(db.raw_conn(), "nightly").unwrap();
    assert_eq!(
        row.last_triggered_at,
        Some(2_000),
        "second upsert must overwrite last_triggered_at via COALESCE"
    );
}

#[test]
fn upsert_preserves_existing_last_triggered_at_when_new_is_none() {
    // The COALESCE has to go BOTH ways: a follow-up upsert with
    // last_triggered_at = None must NOT clobber a previously-set
    // timestamp. (paavo-web's /schedule would otherwise lose history
    // any time some other code path called upsert without bothering
    // to re-populate the timestamps.)
    let db = fresh_db();
    ScheduleRow::upsert(
        db.raw_conn(),
        &ScheduleRow {
            id: "nightly".into(),
            cron: "0 0 19 * * *".into(),
            enabled: true,
            last_triggered_at: Some(7_777),
            last_completed_at: Some(8_888),
        },
    )
    .unwrap();
    ScheduleRow::upsert(
        db.raw_conn(),
        &ScheduleRow {
            id: "nightly".into(),
            cron: "30 0 12 * * *".into(),
            enabled: false,
            last_triggered_at: None,
            last_completed_at: None,
        },
    )
    .unwrap();
    let row = ScheduleRow::get(db.raw_conn(), "nightly").unwrap();
    assert_eq!(row.cron, "30 0 12 * * *");
    assert!(!row.enabled);
    assert_eq!(row.last_triggered_at, Some(7_777));
    assert_eq!(row.last_completed_at, Some(8_888));
}
