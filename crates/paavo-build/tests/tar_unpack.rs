use paavo_build::tar::{blake3_hex, unpack_into};
use tempfile::tempdir;

fn build_sample_tar() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        let body = b"fn main() { println!(\"hi\"); }\n";
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("hello/src/main.rs").unwrap();
        hdr.set_size(body.len() as u64);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        builder.append(&hdr, &body[..]).unwrap();

        let manifest = b"[package]\nname = \"hello\"\nversion = \"0.1.0\"\n";
        let mut hdr2 = tar::Header::new_gnu();
        hdr2.set_path("hello/Cargo.toml").unwrap();
        hdr2.set_size(manifest.len() as u64);
        hdr2.set_mode(0o644);
        hdr2.set_cksum();
        builder.append(&hdr2, &manifest[..]).unwrap();

        builder.finish().unwrap();
    }
    buf
}

#[test]
fn unpack_extracts_all_entries() {
    let dir = tempdir().unwrap();
    let tar = build_sample_tar();
    let dst = unpack_into(&tar, dir.path()).unwrap();
    assert!(dst.join("hello/src/main.rs").is_file());
    assert!(dst.join("hello/Cargo.toml").is_file());
}

#[test]
fn unpack_rejects_path_escape() {
    let dir = tempdir().unwrap();
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        let mut hdr = tar::Header::new_gnu();
        // `set_path` refuses `..` components, so write the raw bytes directly
        // into the header's name field to simulate a malicious archive that
        // bypasses the standard tar builder validation.
        let name = b"../escape.rs";
        let raw = &mut hdr.as_old_mut().name;
        raw[..name.len()].copy_from_slice(name);
        hdr.set_size(0);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        builder.append(&hdr, &[][..]).unwrap();
        builder.finish().unwrap();
    }
    let err = unpack_into(&buf, dir.path()).unwrap_err();
    assert!(
        matches!(
            err,
            paavo_build::BuildError::PathEscape {
                reason: "parent-dir",
                ..
            }
        ),
        "expected PathEscape{{reason: 'parent-dir'}}, got: {err:?}"
    );
}

#[test]
fn blake3_hex_is_deterministic() {
    let a = build_sample_tar();
    let b = build_sample_tar();
    let ha = blake3_hex(&a);
    let hb = blake3_hex(&b);
    assert_eq!(ha, hb);
    assert_eq!(ha.len(), 64); // hex blake3
}
