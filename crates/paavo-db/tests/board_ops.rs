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
