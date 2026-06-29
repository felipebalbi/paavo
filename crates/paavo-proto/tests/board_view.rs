use paavo_proto::{BoardHealth, BoardSpec, BoardView, ProbeSelector};

#[test]
fn board_view_round_trips_through_json() {
    let view = BoardView {
        spec: BoardSpec {
            id: "x".into(),
            kind: "mcxa266".into(),
            probe_selector: ProbeSelector {
                vid: "1366".into(),
                pid: "1015".into(),
                serial: "ABC".into(),
                interface: None,
            },
            chip_name: "X".into(),
            target_name: "T".into(),
            wiring_profile: Some("default".into()),
            health: BoardHealth::Quarantined,
        },
        quarantine_reason: Some("flaky".into()),
        consecutive_infra_failures: 3,
        last_used_at: Some(42),
        created_at: 7,
    };
    let j = serde_json::to_value(&view).unwrap();
    // Flatten: `id` is at the top level alongside `quarantine_reason`.
    assert_eq!(j["id"], "x");
    assert_eq!(j["quarantine_reason"], "flaky");
    assert_eq!(j["consecutive_infra_failures"], 3);
    assert_eq!(j["last_used_at"], 42);
    assert_eq!(j["created_at"], 7);

    let back: BoardView = serde_json::from_value(j).unwrap();
    assert_eq!(back, view);
}

#[test]
fn board_view_omits_none_quarantine_reason() {
    let view = BoardView {
        spec: BoardSpec {
            id: "x".into(),
            kind: "mcxa266".into(),
            probe_selector: ProbeSelector {
                vid: "1".into(),
                pid: "2".into(),
                serial: "S".into(),
                interface: None,
            },
            chip_name: "X".into(),
            target_name: "T".into(),
            wiring_profile: None,
            health: BoardHealth::Healthy,
        },
        quarantine_reason: None,
        consecutive_infra_failures: 0,
        last_used_at: None,
        created_at: 0,
    };
    let j = serde_json::to_value(&view).unwrap();
    // BoardView's own Option fields use skip_serializing_if.
    assert!(j.get("quarantine_reason").is_none());
    assert!(j.get("last_used_at").is_none());
    // BoardSpec::wiring_profile does NOT use skip_serializing_if (the
    // field's serde attrs live on `BoardSelector::wiring_profile`,
    // not on `BoardSpec`), so it serializes as `null` here. Pin the
    // explicit null so a future skip_serializing_if change is caught.
    assert!(j["wiring_profile"].is_null());
}
