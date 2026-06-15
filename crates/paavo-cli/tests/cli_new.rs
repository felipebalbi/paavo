//! Integration tests for `paavo-cli new`. See spec §10.5 and plan
//! task 7.2 for the behaviour contract.

use assert_cmd::Command;
use predicates::prelude::*;

/// Build a PATH that does NOT contain cargo-generate, so the missing-
/// dependency pre-flight path is exercised deterministically regardless
/// of whether the developer has `cargo install cargo-generate`-ed.
///
/// Strategy: find every cargo-generate binary on PATH, collect the
/// containing directories, and build a new PATH with those directories
/// removed. On Windows this is `;`-separated; on Unix, `:`.
fn path_without_cargo_generate() -> std::ffi::OsString {
    use std::collections::HashSet;
    let cg_binaries: HashSet<std::path::PathBuf> = which::which_all("cargo-generate")
        .map(|it| {
            it.filter_map(|p| p.parent().map(|d| d.to_path_buf()))
                .collect()
        })
        .unwrap_or_default();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let kept: Vec<_> = std::env::split_paths(&path)
        .filter(|d| !cg_binaries.contains(d))
        .collect();
    std::env::join_paths(kept).expect("rejoin PATH")
}

#[test]
fn new_without_cargo_generate_errors_clearly() {
    // Exercise the missing-binary pre-flight by stripping cargo-generate
    // from PATH for this one subprocess. This is the only way to make
    // the test reliable on every dev box: a bare `which::which` check
    // would self-skip when cargo-generate is installed (i.e. precisely
    // the environment where 7.2 was developed and where regressions
    // are most likely).
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .env("PATH", path_without_cargo_generate())
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
fn new_with_non_kebab_name_errors_before_touching_filesystem() {
    // cargo-generate would silently kebab `MyTest` to `my-test` and
    // our success-print would lie about `cd MyTest`. The fix is to
    // refuse non-kebab names up front. See cmd_new::validate_kebab_name.
    Command::cargo_bin("paavo-cli")
        .unwrap()
        .args(["new", "MyTest", "--board-kind", "mcxa266"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("lowercase letter"))
        .stderr(predicate::str::contains("MyTest"));
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
