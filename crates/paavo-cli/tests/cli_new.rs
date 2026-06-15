//! Integration tests for `paavo-cli new`. See spec §10.5 and plan
//! task 7.2 for the behaviour contract.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn new_without_cargo_generate_errors_clearly() {
    // Skip if cargo-generate IS available — this test covers the
    // "missing dependency" branch only.
    if which::which("cargo-generate").is_ok() {
        eprintln!("cargo-generate IS installed; skipping missing-dep test");
        return;
    }
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .args(["new", "hello", "--board-kind", "mcxa266"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("cargo-generate not found on PATH"))
        .stderr(predicate::str::contains("cargo install cargo-generate"));
}

#[test]
fn new_with_unknown_board_kind_errors_with_kinds_list() {
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .args(["new", "hello", "--board-kind", "bogus-xyz"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown board kind: bogus-xyz"))
        .stderr(predicate::str::contains("mcxa266"));
}

#[test]
#[ignore] // gated under PAAVO_HW=1 because it does a real cargo-generate +
          // real cargo check against thumbv8m.main-none-eabihf, which is
          // slow and requires the target to be installed.
fn new_mcxa266_scaffolds_and_typechecks() {
    if std::env::var("PAAVO_HW").is_err() {
        eprintln!("PAAVO_HW not set; skipping");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .args([
            "new",
            "smoke-test",
            "--board-kind",
            "mcxa266",
            "--into",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let scaffolded = tmp.path().join("smoke-test");
    assert!(
        scaffolded.join("Cargo.toml").is_file(),
        "Cargo.toml missing"
    );
    assert!(scaffolded.join("src/main.rs").is_file(), "main.rs missing");
    assert!(scaffolded.join("memory.x").is_file(), "memory.x missing");

    // `cargo check` (not build) against thumbv8m.main-none-eabihf.
    // We don't link or download crates we don't need; check stops at
    // typeck which exercises feature-flag correctness.
    let out = std::process::Command::new("cargo")
        .args(["check", "--target", "thumbv8m.main-none-eabihf"])
        .current_dir(&scaffolded)
        .output()
        .expect("spawn cargo check");
    assert!(
        out.status.success(),
        "cargo check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
