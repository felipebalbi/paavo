//! Drives `cargo build --release` against a tiny host crate fixture. The
//! point of this test is to exercise the invocation path; it does *not*
//! cross-compile to an embedded target (CI does not have `thumbv*` linkers
//! installed). The fixture is a `cdylib`-less binary so the discovery path
//! picks up the host triple's release dir.

use paavo_build::{build_release, BuildPlan};
use std::fs;
#[cfg(not(windows))]
use std::path::PathBuf;
use tempfile::tempdir;

#[cfg(not(windows))]
fn write_fixture(root: &std::path::Path) {
    let crate_dir = root.join("hello");
    fs::create_dir_all(crate_dir.join("src")).unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        r#"[package]
name = "hello"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "hello"
path = "src/main.rs"
"#,
    )
    .unwrap();
    fs::write(
        crate_dir.join("src").join("main.rs"),
        r#"fn main() { println!("hi"); }"#,
    )
    .unwrap();
}

// Skipped on Windows: the host target produces a PE/COFF `.exe`, not an
// ELF, so `discover_elf` (which checks the ELF magic) correctly returns
// `NoElf`. The build invocation itself is exercised on every platform via
// `build_release_captures_stderr_on_failure`. CI runs on Linux where the
// host artifact *is* an ELF, so this assertion still gates the discovery
// handoff on the main supported CI platform.
#[cfg(not(windows))]
#[test]
fn build_release_produces_elf_for_host_target() {
    if std::env::var_os("CARGO").is_none() {
        eprintln!("skipping: CARGO not set in env");
        return;
    }
    let dir = tempdir().unwrap();
    write_fixture(dir.path());
    let plan = BuildPlan {
        crate_dir: dir.path().join("hello"),
        target_dir: dir.path().join("cargo-target"),
        cargo_update_packages: vec![],
    };
    let res = build_release(&plan).unwrap();
    let elf: PathBuf = res.elf_path;
    assert!(elf.is_file(), "expected ELF at {elf:?}");
    assert!(res.elf_size_bytes > 0);
    // Lock down the success-path stderr-capture contract: cargo always
    // writes either "Compiling ..." or "Finished ..." (or both) to stderr.
    // If a future refactor accidentally drops the stderr_tail wiring, this
    // assertion will catch it.
    assert!(
        res.stderr_tail.contains("Compiling") || res.stderr_tail.contains("Finished"),
        "expected cargo stderr in tail, got: {:?}",
        res.stderr_tail
    );
}

#[test]
fn build_release_captures_stderr_on_failure() {
    if std::env::var_os("CARGO").is_none() {
        eprintln!("skipping: CARGO not set in env");
        return;
    }
    let dir = tempdir().unwrap();
    let crate_dir = dir.path().join("broken");
    fs::create_dir_all(crate_dir.join("src")).unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        r#"[package]
name = "broken"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    fs::write(
        crate_dir.join("src").join("main.rs"),
        r#"fn main() { compile_error!("kaboom"); }"#,
    )
    .unwrap();
    let plan = BuildPlan {
        crate_dir,
        target_dir: dir.path().join("cargo-target"),
        cargo_update_packages: vec![],
    };
    let err = build_release(&plan).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("kaboom") || msg.contains("compile_error"),
        "{msg}"
    );
}
