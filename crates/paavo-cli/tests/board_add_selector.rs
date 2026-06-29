//! `board add` must reject a malformed --probe locally, before any network
//! call. We point PAAVO_HOST at an unreachable address; if the command fails
//! with the parse message (not a connection error) we know it never POSTed.

use assert_cmd::Command as AssertCommand;
use predicates::prelude::PredicateBooleanExt;

#[test]
fn board_add_rejects_invalid_probe_before_network() {
    let mut cmd = AssertCommand::cargo_bin("paavo-cli").unwrap();
    cmd.env("PAAVO_HOST", "http://127.0.0.1:1").args([
        "board",
        "add",
        "--kind",
        "mcxa266",
        "--instance",
        "mcxa266-99",
        "--probe",
        "zz:gg:NOSER",
        "--chip",
        "MCXA266VFL",
        "--target",
        "frdm-mcx-a266",
    ]);
    cmd.assert().failure().stderr(
        predicates::str::contains("invalid --probe").and(predicates::str::contains("bad VID")),
    );
}
