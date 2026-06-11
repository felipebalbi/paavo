mod common;

use common::{fresh_db, insert_board};
use paavo_core::{apply_outcome_to_board, QuarantinePolicy};
use paavo_proto::{BoardHealth, JobOutcome, TerminalOutcome, TimeoutReason};

#[test]
fn three_infra_errs_auto_quarantine_the_board() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let policy = QuarantinePolicy {
        consecutive_infra_failures: 3,
    };

    for i in 0..2 {
        let just_quarantined = apply_outcome_to_board(
            db.raw_conn(),
            "mcxa266-01",
            &JobOutcome::Failed(TerminalOutcome::InfraErr {
                stage: "probe_attach".into(),
                message: "boom".into(),
            }),
            true,
            policy,
        )
        .unwrap();
        assert!(!just_quarantined, "iter {i}");
    }
    let just_quarantined = apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::Failed(TerminalOutcome::InfraErr {
            stage: "probe_attach".into(),
            message: "boom".into(),
        }),
        true,
        policy,
    )
    .unwrap();
    assert!(just_quarantined);
    let row = paavo_db::BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Quarantined);
    assert!(row
        .quarantine_reason
        .unwrap_or_default()
        .starts_with("auto: 3"));
}

#[test]
fn a_passing_run_resets_the_counter() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let policy = QuarantinePolicy {
        consecutive_infra_failures: 3,
    };

    apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::Failed(TerminalOutcome::InfraErr {
            stage: "x".into(),
            message: "x".into(),
        }),
        true,
        policy,
    )
    .unwrap();
    apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::Passed,
        true,
        policy,
    )
    .unwrap();
    let row = paavo_db::BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.consecutive_infra_failures, 0);
    assert_eq!(
        row.spec.health,
        BoardHealth::Healthy,
        "reset path must leave the board Healthy"
    );
}

#[test]
fn inactivity_timeout_with_unreleased_probe_counts_toward_quarantine() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let policy = QuarantinePolicy {
        consecutive_infra_failures: 1,
    };

    let just_q = apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::TimedOut {
            reason: TimeoutReason::Inactivity,
            elapsed_ms: 120_000,
        },
        /* probe_released_cleanly = */ false,
        policy,
    )
    .unwrap();
    assert!(just_q);
}

#[test]
fn inactivity_timeout_with_clean_probe_release_does_not_count() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let policy = QuarantinePolicy {
        consecutive_infra_failures: 1,
    };

    let just_q = apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::TimedOut {
            reason: TimeoutReason::Inactivity,
            elapsed_ms: 120_000,
        },
        /* probe_released_cleanly = */ true,
        policy,
    )
    .unwrap();
    assert!(!just_q);
    let row = paavo_db::BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Healthy);
}

#[test]
fn hard_max_does_not_count_toward_quarantine() {
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let policy = QuarantinePolicy {
        consecutive_infra_failures: 1,
    };

    let just_q = apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::TimedOut {
            reason: TimeoutReason::HardMax,
            elapsed_ms: 900_000,
        },
        false, // even with bad release
        policy,
    )
    .unwrap();
    assert!(!just_q);
}

#[test]
fn failed_testerr_does_not_count_toward_quarantine() {
    // CRITICAL invariant (spec §5.2): a buggy test must not flake out the
    // board. Only Failed(InfraErr) bumps the counter. A future refactor of
    // counts_toward_infra_failure that simplifies the match to Failed(_)
    // would fail this test.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let policy = QuarantinePolicy {
        consecutive_infra_failures: 1, // single event would quarantine if it counted
    };

    let just_q = apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::Failed(TerminalOutcome::TestErr {
            message: "assertion failed".into(),
        }),
        true, // probe_released_cleanly doesn't matter for TestErr
        policy,
    )
    .unwrap();
    assert!(!just_q, "TestErr must NOT auto-quarantine");

    let row = paavo_db::BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Healthy);
    assert_eq!(row.consecutive_infra_failures, 0);
}

#[test]
fn failed_builderr_does_not_count_toward_quarantine() {
    // Build errors are the submitter's fault, not the board's. Same
    // critical invariant as the TestErr test.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);
    let policy = QuarantinePolicy {
        consecutive_infra_failures: 1,
    };

    let just_q = apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::Failed(TerminalOutcome::BuildErr {
            stderr: "error[E0277]: ...".into(),
        }),
        true,
        policy,
    )
    .unwrap();
    assert!(!just_q, "BuildErr must NOT auto-quarantine");

    let row = paavo_db::BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Healthy);
    assert_eq!(row.consecutive_infra_failures, 0);
}

#[test]
fn aborted_does_not_count_toward_quarantine() {
    // User cancellation must not penalize the board. The current rule
    // routes Aborted through the reset path (counter -> 0), which this
    // test locks down.
    let db = fresh_db();
    insert_board(&db, "mcxa266-01", "mcxa266", BoardHealth::Healthy);

    // Seed the counter with one InfraErr so we can prove the reset.
    apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::Failed(TerminalOutcome::InfraErr {
            stage: "probe_attach".into(),
            message: "boom".into(),
        }),
        true,
        QuarantinePolicy {
            consecutive_infra_failures: 5,
        }, // far above current count
    )
    .unwrap();
    let row = paavo_db::BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.consecutive_infra_failures, 1);

    // Aborted (user cancel) should reset.
    let just_q = apply_outcome_to_board(
        db.raw_conn(),
        "mcxa266-01",
        &JobOutcome::Aborted {
            by: paavo_proto::AbortReason::User,
        },
        true,
        QuarantinePolicy {
            consecutive_infra_failures: 5,
        },
    )
    .unwrap();
    assert!(!just_q);
    let row = paavo_db::BoardRow::get(db.raw_conn(), "mcxa266-01").unwrap();
    assert_eq!(row.consecutive_infra_failures, 0, "Aborted should reset");
    assert_eq!(row.spec.health, BoardHealth::Healthy);
}
