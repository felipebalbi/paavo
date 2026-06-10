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
    // Right now this surfaces as DbError::Sqlite(QueryReturnedNoRows).
    // The exact variant is an implementation detail; just confirm we get an error.
    assert!(
        matches!(err, paavo_db::DbError::Sqlite(_)),
        "missing id should produce DbError::Sqlite(QueryReturnedNoRows), got {err:?}"
    );
}
