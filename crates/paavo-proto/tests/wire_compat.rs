//! Byte-level wire-compat tests for `WireMessage`.
//!
//! These tests pin the JSON shape that `paavod` emits today and that
//! `paavo-cli` parses today, so a future serde upgrade or variant
//! shuffle that silently changes the bytes flips this suite from
//! green to red instead of breaking deployed `paavo-cli` clients in
//! the field.
//!
//! The historical wire shape was hand-rolled with
//! `serde_json::json!({"type":"frame","frame": ...})`-style macros
//! inside `paavod::routes::jobs::stream_job`. These tests check that
//! the typed [`WireMessage`] enum round-trips against the SAME byte
//! strings, with the SAME field-key order, so commit D's switch from
//! `serde_json::json!` to `serde_json::to_string(&WireMessage::*)`
//! is wire-byte-identical and does not break anyone's pinned client.

use paavo_proto::{
    AbortReason, JobOutcome, JobPhase, LogFrame, LogLevel, TerminalOutcome, TimeoutReason,
    WireMessage,
};

/// Helper: assert the value serialises to exactly `expected` and
/// also deserialises back to an equal value.
fn assert_roundtrip(msg: WireMessage, expected: &str) {
    let actual = serde_json::to_string(&msg).expect("serialize");
    assert_eq!(
        actual, expected,
        "wire bytes drifted; expected {expected:?}, got {actual:?}"
    );
    let parsed: WireMessage = serde_json::from_str(expected).expect("deserialise the pinned bytes");
    assert_eq!(
        parsed, msg,
        "round-trip lost data; serialised {expected:?} but parsed differently"
    );
}

// =============================================================
// Variant: Frame
// =============================================================

#[test]
fn frame_with_target_matches_historical_bytes() {
    let frame = LogFrame {
        seq: 42,
        ts_us: 12345,
        level: LogLevel::Info,
        target: Some("cargo:stderr".into()),
        message: "   Compiling foo v0.1.0".into(),
    };
    // Historical wire shape (paavod's hand-rolled json! macro):
    // {"type":"frame","frame":{"seq":42,"ts_us":12345,"level":"info","target":"cargo:stderr","message":"   Compiling foo v0.1.0"}}
    let expected = r#"{"type":"frame","frame":{"seq":42,"ts_us":12345,"level":"info","target":"cargo:stderr","message":"   Compiling foo v0.1.0"}}"#;
    assert_roundtrip(WireMessage::Frame { frame }, expected);
}

#[test]
fn frame_without_target_omits_field() {
    // The historical shape did NOT emit `target` when it was None
    // (LogFrame uses `skip_serializing_if = "Option::is_none"`).
    // Confirm the typed enum preserves that behaviour: an absent
    // target on the wire deserialises back to `None`.
    let frame = LogFrame {
        seq: 0,
        ts_us: 0,
        level: LogLevel::Info,
        target: None,
        message: "Test OK".into(),
    };
    let expected =
        r#"{"type":"frame","frame":{"seq":0,"ts_us":0,"level":"info","message":"Test OK"}}"#;
    assert_roundtrip(WireMessage::Frame { frame }, expected);
}

#[test]
fn frame_levels_serialise_lowercase() {
    // Each level was lowercase on the wire (LogLevel uses
    // rename_all = "lowercase"). Pin all five so a future enum
    // reorder that flipped a serde rename can't sneak through.
    for (level, expected_token) in [
        (LogLevel::Trace, "trace"),
        (LogLevel::Debug, "debug"),
        (LogLevel::Info, "info"),
        (LogLevel::Warn, "warn"),
        (LogLevel::Error, "error"),
    ] {
        let frame = LogFrame {
            seq: 1,
            ts_us: 2,
            level,
            target: None,
            message: "x".into(),
        };
        let s = serde_json::to_string(&WireMessage::Frame { frame }).unwrap();
        assert!(
            s.contains(&format!(r#""level":"{expected_token}""#)),
            "{level:?} did not serialise as {expected_token:?}; full body: {s}"
        );
    }
}

// =============================================================
// Variant: Terminal — every JobOutcome shape
// =============================================================

#[test]
fn terminal_passed_serialises_as_bare_string() {
    // JobOutcome::Passed serialises as the bare string "passed",
    // not as an object. The internal-tagging on the outer enum
    // doesn't change that — `outcome` is a JSON value slot, and a
    // string is a perfectly valid value for it.
    let expected = r#"{"type":"terminal","outcome":"passed"}"#;
    assert_roundtrip(
        WireMessage::Terminal {
            outcome: JobOutcome::Passed,
        },
        expected,
    );
}

#[test]
fn terminal_failed_build_err_matches_historical() {
    let expected = r#"{"type":"terminal","outcome":{"failed":{"kind":"build_err","stderr":"error[E0425]: cannot find value `foo` in this scope"}}}"#;
    assert_roundtrip(
        WireMessage::Terminal {
            outcome: JobOutcome::Failed(TerminalOutcome::BuildErr {
                stderr: "error[E0425]: cannot find value `foo` in this scope".into(),
            }),
        },
        expected,
    );
}

#[test]
fn terminal_failed_test_err_matches_historical() {
    let expected = r#"{"type":"terminal","outcome":{"failed":{"kind":"test_err","message":"assertion failed"}}}"#;
    assert_roundtrip(
        WireMessage::Terminal {
            outcome: JobOutcome::Failed(TerminalOutcome::TestErr {
                message: "assertion failed".into(),
            }),
        },
        expected,
    );
}

#[test]
fn terminal_failed_infra_err_matches_historical() {
    let expected = r#"{"type":"terminal","outcome":{"failed":{"kind":"infra_err","stage":"probe_attach","message":"USB device not found"}}}"#;
    assert_roundtrip(
        WireMessage::Terminal {
            outcome: JobOutcome::Failed(TerminalOutcome::InfraErr {
                stage: "probe_attach".into(),
                message: "USB device not found".into(),
            }),
        },
        expected,
    );
}

#[test]
fn terminal_timedout_inactivity_matches_historical() {
    let expected = r#"{"type":"terminal","outcome":{"timed_out":{"reason":"inactivity","elapsed_ms":120000}}}"#;
    assert_roundtrip(
        WireMessage::Terminal {
            outcome: JobOutcome::TimedOut {
                reason: TimeoutReason::Inactivity,
                elapsed_ms: 120000,
            },
        },
        expected,
    );
}

#[test]
fn terminal_timedout_hard_max_matches_historical() {
    let expected =
        r#"{"type":"terminal","outcome":{"timed_out":{"reason":"hard_max","elapsed_ms":900000}}}"#;
    assert_roundtrip(
        WireMessage::Terminal {
            outcome: JobOutcome::TimedOut {
                reason: TimeoutReason::HardMax,
                elapsed_ms: 900000,
            },
        },
        expected,
    );
}

#[test]
fn terminal_aborted_user_matches_historical() {
    let expected = r#"{"type":"terminal","outcome":{"aborted":{"by":"user"}}}"#;
    assert_roundtrip(
        WireMessage::Terminal {
            outcome: JobOutcome::Aborted {
                by: AbortReason::User,
            },
        },
        expected,
    );
}

#[test]
fn terminal_aborted_daemon_shutdown_matches_historical() {
    let expected = r#"{"type":"terminal","outcome":{"aborted":{"by":"daemon_shutdown"}}}"#;
    assert_roundtrip(
        WireMessage::Terminal {
            outcome: JobOutcome::Aborted {
                by: AbortReason::DaemonShutdown,
            },
        },
        expected,
    );
}

// =============================================================
// Variants: Lagged + Truncated
// =============================================================

#[test]
fn lagged_matches_historical_bytes() {
    let expected = r#"{"type":"lagged","missed":7}"#;
    assert_roundtrip(WireMessage::Lagged { missed: 7 }, expected);
}

#[test]
fn truncated_matches_historical_bytes() {
    let expected = r#"{"type":"truncated","reason":"live stream ended before terminal"}"#;
    assert_roundtrip(
        WireMessage::Truncated {
            reason: "live stream ended before terminal".into(),
        },
        expected,
    );
}

// =============================================================
// Variant: Phase (NEW)
// =============================================================

#[test]
fn phase_building_serialises_as_lowercase() {
    let expected = r#"{"type":"phase","phase":"building"}"#;
    assert_roundtrip(
        WireMessage::Phase {
            phase: JobPhase::Building,
        },
        expected,
    );
}

#[test]
fn phase_running_serialises_as_lowercase() {
    let expected = r#"{"type":"phase","phase":"running"}"#;
    assert_roundtrip(
        WireMessage::Phase {
            phase: JobPhase::Running,
        },
        expected,
    );
}

// =============================================================
// Forward-compat: unknown `type` values are an Err, not a panic
// =============================================================

#[test]
fn unknown_type_value_is_a_recoverable_deserialise_error() {
    // The forward-compat contract: a future paavod that emits a new
    // variant (e.g. `{"type":"checkpoint","at":42}`) MUST surface to
    // older paavo-cli/paavo-web as a deserialisation error that
    // consumers can `eprintln!` and continue past — never as a panic.
    let unknown = r#"{"type":"checkpoint","at":42}"#;
    let result: Result<WireMessage, _> = serde_json::from_str(unknown);
    assert!(
        result.is_err(),
        "expected unknown variant to fail deserialisation; got {result:?}"
    );
    // Sanity: known variants still parse against the same code path.
    let known = r#"{"type":"phase","phase":"running"}"#;
    let parsed: WireMessage = serde_json::from_str(known).unwrap();
    assert_eq!(
        parsed,
        WireMessage::Phase {
            phase: JobPhase::Running
        }
    );
}

// =============================================================
// closes_stream invariant
// =============================================================

#[test]
fn closes_stream_for_terminal_and_truncated_only() {
    let frame = LogFrame {
        seq: 0,
        ts_us: 0,
        level: LogLevel::Info,
        target: None,
        message: "x".into(),
    };
    assert!(!WireMessage::Frame { frame }.closes_stream());
    assert!(!WireMessage::Lagged { missed: 1 }.closes_stream());
    assert!(!WireMessage::Phase {
        phase: JobPhase::Running
    }
    .closes_stream());
    assert!(WireMessage::Truncated { reason: "x".into() }.closes_stream());
    assert!(WireMessage::Terminal {
        outcome: JobOutcome::Passed
    }
    .closes_stream());
}
