use paavo_build::{discover_elf, ManifestArtifactHint};
use std::fs;
use tempfile::tempdir;

fn make_elf(path: &std::path::Path) {
    // Minimal valid-ish ELF prefix (magic + class + endian). The discovery
    // logic only verifies the magic.
    let mut bytes = vec![0x7f, b'E', b'L', b'F', 2, 1, 1];
    bytes.extend(std::iter::repeat_n(0u8, 57)); // pad to typical Ehdr size
    fs::write(path, &bytes).unwrap();
}

#[test]
fn picks_up_hinted_artifact_dir() {
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let artifact_dir = crate_dir.join("artifacts");
    fs::create_dir_all(&artifact_dir).unwrap();
    make_elf(&artifact_dir.join("hello.elf"));

    let elf = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint {
            artifact_dir: Some("artifacts".into()),
        },
    )
    .unwrap();
    assert_eq!(elf, artifact_dir.join("hello.elf"));
}

#[test]
fn scans_target_release_when_no_hint() {
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let release = crate_dir
        .join("target")
        .join("thumbv8m.main-none-eabihf")
        .join("release");
    fs::create_dir_all(&release).unwrap();
    make_elf(&release.join("hello"));
    make_elf(&release.join("hello.elf"));

    let elf = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint::default(),
    )
    .unwrap();
    // Prefer the explicit .elf extension when both are present.
    assert_eq!(elf, release.join("hello.elf"));
}

#[test]
fn scans_host_target_release_dir() {
    // Host builds (no cross-compile) write directly to target/release/,
    // not target/<triple>/release/. Task 3.1.c's `build_release` test
    // depends on this path being recognized.
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let release = crate_dir.join("target").join("release");
    fs::create_dir_all(&release).unwrap();
    make_elf(&release.join("hello"));

    let elf = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint::default(),
    )
    .unwrap();
    assert_eq!(elf, release.join("hello"));
}

#[test]
fn errors_when_no_elf_present() {
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let release = crate_dir
        .join("target")
        .join("thumbv8m.main-none-eabihf")
        .join("release");
    fs::create_dir_all(&release).unwrap();

    let err = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint::default(),
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("no ELF"), "{msg}");
}

#[test]
fn errors_when_hint_dir_does_not_exist() {
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();

    let err = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint {
            artifact_dir: Some("nonexistent-out".into()),
        },
    )
    .unwrap_err();

    assert!(
        matches!(err, paavo_build::BuildError::HintDirMissing { .. }),
        "expected HintDirMissing, got: {err:?}"
    );
}

#[test]
fn errors_when_hint_dir_is_empty() {
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let hint_dir = crate_dir.join("artifacts");
    fs::create_dir_all(&hint_dir).unwrap();
    // Directory exists but contains no ELF files.

    let err = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint {
            artifact_dir: Some("artifacts".into()),
        },
    )
    .unwrap_err();

    let msg = format!("{err}");
    assert!(msg.contains("no ELF"), "{msg}");
}

#[test]
fn pick_elf_finds_nested_artifact_under_release_deps() {
    // Locks down the depth=3 walk contract: a typical cargo cross-build
    // produces target/<triple>/release/deps/<crate>-<hash>.elf, which is
    // depth=2 from the release/ scan-root. This test exercises that path.
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let release = crate_dir
        .join("target")
        .join("thumbv8m.main-none-eabihf")
        .join("release");
    let deps = release.join("deps");
    fs::create_dir_all(&deps).unwrap();
    make_elf(&deps.join("hello-abc123.elf"));

    let elf = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint::default(),
    )
    .unwrap();
    assert_eq!(elf, deps.join("hello-abc123.elf"));
}

#[test]
fn triple_subdir_wins_over_bare_release_when_both_exist() {
    // Regression: a cross-compiled crate that has proc-macro deps
    // (e.g. cortex-m-rt -> cortex-m-rt-macros) makes cargo populate
    // both `target/release/` (host-built proc-macro .rlib/.dll files,
    // no ELFs) AND `target/<triple>/release/` (the actual artifact).
    // Discovery must prefer the triple subdir; the older "host wins"
    // precedence picked the bare release/, found no ELF magic among
    // the proc-macro outputs, and surfaced "no ELF found" even though
    // the artifact existed one level over.
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let bare_release = crate_dir.join("target").join("release");
    fs::create_dir_all(bare_release.join("deps")).unwrap();
    // Drop a non-ELF file into the bare release dir to mimic a
    // host-built proc-macro output. Magic intentionally NOT 0x7F ELF.
    fs::write(
        bare_release.join("deps").join("libproc_macro2.rlib"),
        b"!<arch>\n",
    )
    .unwrap();

    let triple_release = crate_dir
        .join("target")
        .join("thumbv8m.main-none-eabihf")
        .join("release");
    fs::create_dir_all(&triple_release).unwrap();
    make_elf(&triple_release.join("hello"));

    let elf = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint::default(),
    )
    .unwrap();
    assert_eq!(elf, triple_release.join("hello"));
}

#[test]
fn bare_release_used_when_triple_subdir_has_no_elf() {
    // Fallback path: if every triple subdir is empty of ELFs (e.g.
    // a crate built only for host, and a vestigial triple dir exists
    // from a previous configuration), bare release/ is still tried.
    // Defends against an over-eager refactor that drops the fallback.
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let triple_release = crate_dir
        .join("target")
        .join("thumbv8m.main-none-eabihf")
        .join("release");
    fs::create_dir_all(&triple_release).unwrap();
    // No ELF in the triple dir.

    let bare_release = crate_dir.join("target").join("release");
    fs::create_dir_all(&bare_release).unwrap();
    make_elf(&bare_release.join("hello"));

    let elf = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint::default(),
    )
    .unwrap();
    assert_eq!(elf, bare_release.join("hello"));
}

#[test]
fn noelf_error_lists_scanned_roots_and_file_count() {
    // When no ELF turns up under any candidate root, the error must
    // tell the operator (a) which roots were searched and (b) how many
    // files were inspected, so they can tell "discovery is looking in
    // the wrong place" from "cargo built nothing that looks like an
    // ELF". This was a silent failure mode pre-Round-2: the operator
    // saw "no ELF found in <bare release>" and had no signal that the
    // triple subdir existed but was empty.
    let dir = tempdir().unwrap();
    let crate_dir = dir.path();
    let bare_release = crate_dir.join("target").join("release");
    let triple_release = crate_dir
        .join("target")
        .join("thumbv8m.main-none-eabihf")
        .join("release");
    fs::create_dir_all(&bare_release).unwrap();
    fs::create_dir_all(&triple_release).unwrap();
    // Sprinkle some non-ELF files so `scanned` is meaningfully non-zero.
    fs::write(bare_release.join("a.rlib"), b"!<arch>\n").unwrap();
    fs::write(triple_release.join("b.exe"), b"MZ\x00\x00").unwrap();

    let err = discover_elf(
        crate_dir,
        &crate_dir.join("target"),
        &ManifestArtifactHint::default(),
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("scanned 2 file"), "got: {msg}");
    assert!(msg.contains("release"), "got: {msg}");
    assert!(msg.contains("cross-compiles"), "got: {msg}");
}
