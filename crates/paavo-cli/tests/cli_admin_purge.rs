//! Confirmation-gate behavior for `paavo-cli admin purge --boards`.
//!
//! Strategy: point the CLI at an unreachable host. Any code path that
//! issues an HTTP request fails to connect and exits non-zero. So a
//! command that *exits 0* must have short-circuited before the request
//! — which is exactly what a declined confirmation should do.

use assert_cmd::Command;
use predicates::str::contains;

/// Nothing listens on TCP port 1; connection is refused immediately.
const DEAD_HOST: &str = "http://127.0.0.1:1";

#[test]
fn purge_boards_declined_aborts_without_calling_server() {
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .env("PAAVO_HOST", DEAD_HOST)
        .args(["admin", "purge", "--boards"])
        .write_stdin("n\n")
        .assert()
        .success()
        .stdout(contains("aborted"));
}

#[test]
fn purge_boards_empty_answer_aborts() {
    // EOF / empty line must be treated as "no".
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .env("PAAVO_HOST", DEAD_HOST)
        .args(["admin", "purge", "--boards"])
        .write_stdin("")
        .assert()
        .success()
        .stdout(contains("aborted"));
}

#[test]
fn purge_boards_yes_flag_bypasses_prompt_and_reaches_network() {
    // --yes skips the prompt, so the command proceeds to the HTTP call
    // and fails to connect to the dead host → non-zero exit. That
    // failure is the signal the prompt was bypassed.
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .env("PAAVO_HOST", DEAD_HOST)
        .args(["admin", "purge", "--boards", "--yes"])
        .assert()
        .failure();
}
