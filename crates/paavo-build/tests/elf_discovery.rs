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
