use paavo_proto::{
    BoardHealth, BoardSelector, BoardSpec, JobId, JobOutcome, JobSpec, JobState, LogFrame,
    LogLevel, Priority, TerminalOutcome, TimeoutReason,
};

#[test]
fn job_id_roundtrip() {
    let id = JobId::new();
    let s = serde_json::to_string(&id).unwrap();
    let parsed: JobId = serde_json::from_str(&s).unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn priority_roundtrip() {
    for p in [Priority::Interactive, Priority::Scheduled] {
        let s = serde_json::to_string(&p).unwrap();
        let parsed: Priority = serde_json::from_str(&s).unwrap();
        assert_eq!(p, parsed);
    }
}

#[test]
fn board_selector_roundtrip() {
    let s = BoardSelector {
        kind: "mcxa266".into(),
        instance: Some("mcxa266-02".into()),
        wiring_profile: Some("alt-spi".into()),
    };
    let json = serde_json::to_string(&s).unwrap();
    let parsed: BoardSelector = serde_json::from_str(&json).unwrap();
    assert_eq!(s, parsed);
}

#[test]
fn job_state_roundtrip() {
    let states = [
        JobState::Submitted,
        JobState::Building,
        JobState::Running,
        JobState::Passed,
        JobState::Failed,
        JobState::TimedOut,
        JobState::Aborted,
    ];
    for st in states {
        let s = serde_json::to_string(&st).unwrap();
        let parsed: JobState = serde_json::from_str(&s).unwrap();
        assert_eq!(st, parsed);
    }
}

#[test]
fn job_outcome_roundtrip_all_variants() {
    let outcomes = [
        JobOutcome::Passed,
        JobOutcome::Failed(TerminalOutcome::TestErr {
            message: "assertion failed".into(),
        }),
        JobOutcome::Failed(TerminalOutcome::BuildErr {
            stderr: "E0432".into(),
        }),
        JobOutcome::Failed(TerminalOutcome::InfraErr {
            stage: "probe_attach".into(),
            message: "no probe".into(),
        }),
        JobOutcome::TimedOut {
            reason: TimeoutReason::Inactivity,
            elapsed_ms: 120_000,
        },
        JobOutcome::TimedOut {
            reason: TimeoutReason::HardMax,
            elapsed_ms: 900_000,
        },
        JobOutcome::Aborted {
            by: paavo_proto::AbortReason::User,
        },
        JobOutcome::Aborted {
            by: paavo_proto::AbortReason::DaemonShutdown,
        },
    ];
    for o in outcomes {
        let s = serde_json::to_string(&o).unwrap();
        let parsed: JobOutcome = serde_json::from_str(&s).unwrap();
        assert_eq!(o, parsed);
    }
}

#[test]
fn log_frame_roundtrip() {
    let f = LogFrame {
        seq: 42,
        ts_us: 1_234_567,
        level: LogLevel::Info,
        target: Some("app::dma".into()),
        message: "Test OK".into(),
    };
    let s = serde_json::to_string(&f).unwrap();
    let parsed: LogFrame = serde_json::from_str(&s).unwrap();
    assert_eq!(f, parsed);
}

#[test]
fn job_spec_roundtrip() {
    let spec = JobSpec {
        priority: Priority::Interactive,
        submitter: "felipe".into(),
        board_selector: BoardSelector {
            kind: "mcxa266".into(),
            instance: None,
            wiring_profile: None,
        },
        inactivity_timeout_ms: Some(120_000),
        hard_max_ms: Some(900_000),
    };
    let s = serde_json::to_string(&spec).unwrap();
    let parsed: JobSpec = serde_json::from_str(&s).unwrap();
    assert_eq!(spec, parsed);
}

#[test]
fn job_spec_wire_shape_omits_source_and_tar_blake3() {
    // Spec section 9.1: `source` is server-forced to Cli; tar_blake3
    // is server-computed from the uploaded bytes. paavod's deserializer
    // uses deny_unknown_fields, so any client that includes these
    // fields will get 400. Pin the contract here so a future field-
    // rename can't silently break paavo-cli.
    let spec = JobSpec {
        priority: Priority::Interactive,
        submitter: "felipe".into(),
        board_selector: BoardSelector {
            kind: "mcxa266".into(),
            instance: None,
            wiring_profile: None,
        },
        inactivity_timeout_ms: None,
        hard_max_ms: None,
    };
    let j = serde_json::to_value(&spec).unwrap();
    assert!(j.get("source").is_none(), "JobSpec must not expose source");
    assert!(
        j.get("tar_blake3").is_none(),
        "JobSpec must not expose tar_blake3"
    );
    // Optional None fields are omitted via skip_serializing_if.
    assert!(j.get("inactivity_timeout_ms").is_none());
    assert!(j.get("hard_max_ms").is_none());
}

#[test]
fn board_spec_roundtrip() {
    let b = BoardSpec {
        id: "mcxa266-01".into(),
        kind: "mcxa266".into(),
        probe_selector: paavo_proto::ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: "000123456789".into(),
        },
        chip_name: "MCXA266VFL".into(),
        target_name: "frdm-mcx-a266".into(),
        wiring_profile: Some("default".into()),
        health: BoardHealth::Healthy,
    };
    let s = serde_json::to_string(&b).unwrap();
    let parsed: BoardSpec = serde_json::from_str(&s).unwrap();
    assert_eq!(b, parsed);
}

// ---- Behavioural tests (not just round-trips) ----

#[test]
fn priority_weights_are_ordered_interactive_first() {
    assert_eq!(Priority::Interactive.weight(), 0);
    assert_eq!(Priority::Scheduled.weight(), 1);
    assert!(
        Priority::Interactive.weight() < Priority::Scheduled.weight(),
        "interactive must outrank scheduled (smaller weight wins)"
    );
}

#[test]
fn board_selector_matches_kind_instance_and_wiring_profile() {
    let board = BoardSpec {
        id: "mcxa266-01".into(),
        kind: "mcxa266".into(),
        probe_selector: paavo_proto::ProbeSelector {
            vid: "1366".into(),
            pid: "1015".into(),
            serial: "abc".into(),
        },
        chip_name: "MCXA266VFL".into(),
        target_name: "frdm-mcx-a266".into(),
        wiring_profile: Some("default".into()),
        health: BoardHealth::Healthy,
    };

    // Kind mismatch -> false.
    assert!(!BoardSelector {
        kind: "rt685-evk".into(),
        instance: None,
        wiring_profile: None
    }
    .matches(&board));

    // Kind match, no instance, no profile -> true (any healthy board of kind).
    assert!(BoardSelector {
        kind: "mcxa266".into(),
        instance: None,
        wiring_profile: None
    }
    .matches(&board));

    // Kind + instance match -> true.
    assert!(BoardSelector {
        kind: "mcxa266".into(),
        instance: Some("mcxa266-01".into()),
        wiring_profile: None
    }
    .matches(&board));

    // Kind match + instance mismatch -> false.
    assert!(!BoardSelector {
        kind: "mcxa266".into(),
        instance: Some("mcxa266-02".into()),
        wiring_profile: None
    }
    .matches(&board));

    // Kind match + profile required and equal -> true.
    assert!(BoardSelector {
        kind: "mcxa266".into(),
        instance: None,
        wiring_profile: Some("default".into())
    }
    .matches(&board));

    // Kind match + profile required but mismatch -> false.
    assert!(!BoardSelector {
        kind: "mcxa266".into(),
        instance: None,
        wiring_profile: Some("alt-spi".into())
    }
    .matches(&board));

    // Selector with no profile matches a board that *has* a profile - selector profile is "any if not specified".
    assert!(BoardSelector {
        kind: "mcxa266".into(),
        instance: None,
        wiring_profile: None
    }
    .matches(&board));
}

#[test]
fn job_state_timedout_wire_string_is_one_word() {
    // This pins spec §7.2's SQL CHECK constraint. If you flip the per-variant
    // rename back to default snake_case, this test will fail before the DB
    // layer's INSERT does - and that's the point.
    assert_eq!(
        serde_json::to_string(&JobState::TimedOut).unwrap(),
        "\"timedout\""
    );
    assert_eq!(
        serde_json::to_string(&JobState::Submitted).unwrap(),
        "\"submitted\""
    );
    assert_eq!(
        serde_json::to_string(&JobState::Building).unwrap(),
        "\"building\""
    );
    assert_eq!(
        serde_json::to_string(&JobState::Running).unwrap(),
        "\"running\""
    );
    assert_eq!(
        serde_json::to_string(&JobState::Passed).unwrap(),
        "\"passed\""
    );
    assert_eq!(
        serde_json::to_string(&JobState::Failed).unwrap(),
        "\"failed\""
    );
    assert_eq!(
        serde_json::to_string(&JobState::Aborted).unwrap(),
        "\"aborted\""
    );
}

#[test]
fn job_outcome_wire_strings_pin_external_tagging() {
    // Passed is a bare string.
    assert_eq!(
        serde_json::to_string(&JobOutcome::Passed).unwrap(),
        "\"passed\""
    );

    // TimedOut is snake_case at the outer layer ("timed_out", NOT "timedout" -
    // that asymmetry with JobState is intentional: JobState backs an SQL CHECK
    // constraint, JobOutcome is freeform JSON).
    let s = serde_json::to_string(&JobOutcome::TimedOut {
        reason: TimeoutReason::Inactivity,
        elapsed_ms: 120_000,
    })
    .unwrap();
    assert!(
        s.starts_with(r#"{"timed_out":"#),
        "expected timed_out outer key: {s}"
    );
    assert!(
        s.contains(r#""reason":"inactivity""#),
        "expected inactivity reason: {s}"
    );

    // Failed(InfraErr {...}) - both outer "failed" and inner "kind":"infra_err".
    let s = serde_json::to_string(&JobOutcome::Failed(TerminalOutcome::InfraErr {
        stage: "probe_attach".into(),
        message: "no probe".into(),
    }))
    .unwrap();
    assert!(
        s.starts_with(r#"{"failed":"#),
        "expected failed outer key: {s}"
    );
    assert!(
        s.contains(r#""kind":"infra_err""#),
        "expected inner kind tag: {s}"
    );
}

#[test]
fn job_view_roundtrip() {
    use paavo_proto::*;
    let view = JobView {
        id: JobId::new(),
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
        state: JobState::Running,
        outcome: None,
        board_id: Some("mcxa266-01".into()),
        submitted_at: 1_700_000_000_000,
        started_at: Some(1_700_000_001_000),
        finished_at: None,
        tar_blake3: "deadbeef".into(),
        cargo_update_packages: vec![],
    };
    let json = serde_json::to_value(&view).unwrap();
    // Wire shape contract: no tar_path, no elf_path.
    assert!(
        json.get("tar_path").is_none(),
        "JobView must not expose tar_path"
    );
    assert!(
        json.get("elf_path").is_none(),
        "JobView must not expose elf_path"
    );
    // state uses the JobState snake_case rename ("running").
    assert_eq!(json["state"], "running");
    // None fields are omitted via skip_serializing_if.
    assert!(json.get("outcome").is_none());
    assert!(json.get("finished_at").is_none());

    let back: JobView = serde_json::from_value(json).unwrap();
    assert_eq!(back, view);
}

#[test]
fn job_view_terminal_includes_outcome_and_finished_at() {
    use paavo_proto::*;
    let view = JobView {
        id: JobId::new(),
        priority: Priority::Scheduled,
        submitter: "cron".into(),
        source: JobSource::Scheduler,
        board_selector: BoardSelector {
            kind: "mcxa266".into(),
            instance: None,
            wiring_profile: None,
        },
        inactivity_timeout_ms: 120_000,
        hard_max_ms: 14_400_000,
        state: JobState::Aborted,
        outcome: Some(JobOutcome::Aborted {
            by: AbortReason::User,
        }),
        board_id: None,
        submitted_at: 1,
        started_at: None,
        finished_at: Some(2),
        tar_blake3: "x".into(),
        cargo_update_packages: vec![],
    };
    let json = serde_json::to_value(&view).unwrap();
    assert_eq!(json["state"], "aborted");
    assert_eq!(json["finished_at"], 2);
    assert!(json["outcome"].is_object());
}
