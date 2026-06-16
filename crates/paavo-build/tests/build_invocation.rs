//! Drives `cargo build --release` against a tiny host crate fixture. The
//! point of this test is to exercise the invocation path; it does *not*
//! cross-compile to an embedded target (CI does not have `thumbv*` linkers
//! installed). The fixture is a `cdylib`-less binary so the discovery path
//! picks up the host triple's release dir.

use paavo_build::{build_release, build_release_streaming, BuildLine, BuildPlan, BuildStream};
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

fn write_broken_fixture(root: &std::path::Path) -> std::path::PathBuf {
    let crate_dir = root.join("broken");
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
    crate_dir
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
    let crate_dir = write_broken_fixture(dir.path());
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

#[test]
fn streaming_failure_emits_stderr_lines_to_sink_and_in_tail() {
    // Pin the §3.1 invariant I1 from the architect spec: "Every line
    // that lands in BuildError::Cargo.stderr was sent to the sink
    // before the function returns." A future refactor that returns
    // before the stderr reader thread joins (e.g. on a wait()
    // timeout, or by introducing tokio without proper join ordering)
    // would break this and silently lose the diagnostic from the
    // live stream.
    if std::env::var_os("CARGO").is_none() {
        eprintln!("skipping: CARGO not set in env");
        return;
    }
    let dir = tempdir().unwrap();
    let crate_dir = write_broken_fixture(dir.path());
    let plan = BuildPlan {
        crate_dir,
        target_dir: dir.path().join("cargo-target"),
        cargo_update_packages: vec![],
    };

    let (tx, rx) = crossbeam_channel::unbounded::<BuildLine>();
    let err = build_release_streaming(&plan, tx).unwrap_err();

    // Drain the channel — by I1 every line is already there.
    let lines: Vec<BuildLine> = rx.iter().collect();
    let stderr_text: String = lines
        .iter()
        .filter(|l| l.stream == BuildStream::Stderr)
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        stderr_text.contains("kaboom") || stderr_text.contains("compile_error"),
        "stream missed the diagnostic; got:\n{stderr_text}"
    );

    // The structured error variant also has the line in its tail.
    let paavo_build::BuildError::Cargo { stderr, .. } = err else {
        panic!("expected BuildError::Cargo, got: {err:?}");
    };
    assert!(
        stderr.contains("kaboom") || stderr.contains("compile_error"),
        "stderr tail missed it: {stderr:?}"
    );
}

#[cfg(not(windows))]
#[test]
fn streaming_success_emits_compiling_or_finished_to_sink() {
    // Companion to the failure-path streaming test on the success
    // path: the live stream sees cargo's progress lines, not just
    // the final result. Without this assertion a future refactor
    // that buffered all streaming lines internally and only flushed
    // on cargo exit would pass the failure test (lines arrive
    // eventually) but silently break the "live progress" contract
    // paavo-web and paavo-cli depend on.
    //
    // Tolerant of the host-triple-on-macOS pre-existing quirk: on
    // macOS the host build produces Mach-O (not ELF), so the post-
    // build `discover_elf` step returns `BuildError::NoElf`. That's
    // a separate concern from this test's contract — the streaming
    // surface still emitted progress lines before the ELF check
    // ran. The match arm preserves the assertion on the lines while
    // tolerating NoElf.
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

    let (tx, rx) = crossbeam_channel::unbounded::<BuildLine>();
    let result = build_release_streaming(&plan, tx);
    // tx was moved into build_release_streaming; it (and the two
    // reader threads' clones) are all dropped by now, so this drains
    // every queued line and terminates.
    let lines: Vec<BuildLine> = rx.iter().collect();

    let stderr_text: String = lines
        .iter()
        .filter(|l| l.stream == BuildStream::Stderr)
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        stderr_text.contains("Compiling") || stderr_text.contains("Finished"),
        "expected cargo progress in stream, got:\n{stderr_text}"
    );

    match result {
        Ok(res) => assert!(res.elf_size_bytes > 0),
        Err(paavo_build::BuildError::NoElf { .. }) => {
            // Host produced Mach-O (macOS) instead of ELF; the
            // streaming part is what this test asserts and that
            // already passed above.
            eprintln!(
                "host build produced non-ELF (likely Mach-O on macOS); streaming surface validated"
            );
        }
        Err(other) => panic!("unexpected build error: {other:?}"),
    }
}

#[cfg(not(windows))]
#[test]
fn streaming_back_compat_wrapper_drops_lines_silently() {
    // build_release(plan) == build_release_streaming(plan, tx) where
    // rx is dropped immediately. The "rx is dropped" branch must not
    // crash the build — the reader threads catch the SendError and
    // continue. A regression here would manifest as the wrapper
    // panicking on its first emitted line (or a deadlock if a panic
    // killed a reader thread mid-iteration and left the pipe full).
    //
    // Same macOS-Mach-O tolerance as
    // streaming_success_emits_compiling_or_finished_to_sink: NoElf is
    // a separate concern from the wrapper-doesn't-deadlock contract.
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
    match build_release(&plan) {
        Ok(res) => {
            assert!(res.elf_size_bytes > 0);
            assert!(
                res.stderr_tail.contains("Compiling") || res.stderr_tail.contains("Finished"),
                "expected stderr_tail to be populated even when rx is dropped, got: {:?}",
                res.stderr_tail
            );
        }
        Err(paavo_build::BuildError::NoElf { .. }) => {
            eprintln!(
                "host build produced non-ELF (likely Mach-O on macOS); the wrapper still ran cargo \
                 to completion and didn't deadlock — that's the contract this test pins"
            );
        }
        Err(other) => panic!("unexpected build error: {other:?}"),
    }
}
