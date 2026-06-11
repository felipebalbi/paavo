mod common;

use common::fresh_db;
use paavo_core::{cache_lookup, cache_store, evict_lru, CacheLookup};
use paavo_db::BuildCacheEntry;
use std::fs;
use tempfile::tempdir;

const NOW: i64 = 1_700_000_000_000;

#[test]
fn lookup_returns_miss_when_no_entry() {
    let db = fresh_db();
    assert_eq!(
        cache_lookup(db.raw_conn(), "deadbeef", NOW).unwrap(),
        CacheLookup::Miss
    );
}

#[test]
fn lookup_returns_hit_after_store_and_bumps_last_used() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();
    let elf = tmp.path().join("foo.elf");
    fs::write(&elf, b"\x7fELF").unwrap();

    let blake = "aabbccdd";
    cache_store(db.raw_conn(), blake, &elf, NOW).unwrap();

    match cache_lookup(db.raw_conn(), blake, NOW + 1).unwrap() {
        CacheLookup::Hit { elf_path } => assert_eq!(elf_path, elf),
        CacheLookup::Miss => panic!("expected Hit"),
    }
    let row = BuildCacheEntry::get(db.raw_conn(), blake).unwrap();
    assert_eq!(row.last_used_at, NOW + 1, "lookup must bump last_used_at");
}

#[test]
fn lookup_returns_miss_if_elf_file_disappeared() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();
    let elf = tmp.path().join("foo.elf");
    fs::write(&elf, b"\x7fELF").unwrap();
    let blake = "ff00ff00";
    cache_store(db.raw_conn(), blake, &elf, NOW).unwrap();
    fs::remove_file(&elf).unwrap();

    assert_eq!(
        cache_lookup(db.raw_conn(), blake, NOW + 1).unwrap(),
        CacheLookup::Miss
    );
    // The stale row should have been pruned.
    assert!(BuildCacheEntry::find(db.raw_conn(), blake)
        .unwrap()
        .is_none());
}

#[test]
fn store_records_size_from_filesystem() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();
    let elf = tmp.path().join("foo.elf");
    let payload = b"\x7fELFsomemorebytes";
    fs::write(&elf, payload).unwrap();
    cache_store(db.raw_conn(), "sizetest", &elf, NOW).unwrap();
    let row = BuildCacheEntry::get(db.raw_conn(), "sizetest").unwrap();
    assert_eq!(row.size_bytes, payload.len() as u64);
}

#[test]
fn evict_lru_drops_least_recently_used_entries_and_unlinks_files() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();

    // Three entries with distinct last_used_at; size 100 each = 300 total.
    let mut elfs: Vec<std::path::PathBuf> = Vec::new();
    for (i, blake) in ["aa", "bb", "cc"].iter().enumerate() {
        let path = tmp.path().join(format!("{blake}.elf"));
        fs::write(&path, vec![0u8; 100]).unwrap();
        cache_store(db.raw_conn(), blake, &path, NOW + i as i64).unwrap();
        elfs.push(path);
    }

    // Cap at 200 bytes -> evict the oldest (aa).
    let evicted = evict_lru(db.raw_conn(), 200).unwrap();
    assert_eq!(evicted.len(), 1, "expected exactly one eviction");
    assert_eq!(evicted[0].tar_blake3, "aa");
    assert!(!elfs[0].exists(), "evicted ELF file must be unlinked");
    assert!(elfs[1].exists(), "non-evicted ELF must remain");
    assert!(elfs[2].exists());
}

#[test]
fn evict_lru_with_cap_zero_drops_every_entry() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();
    for blake in ["aa", "bb", "cc"] {
        let p = tmp.path().join(format!("{blake}.elf"));
        fs::write(&p, vec![0u8; 100]).unwrap();
        cache_store(db.raw_conn(), blake, &p, NOW).unwrap();
    }
    let evicted = evict_lru(db.raw_conn(), 0).unwrap();
    assert_eq!(evicted.len(), 3, "cap=0 should evict everything");
}

#[test]
fn evict_lru_with_cap_above_total_is_a_no_op() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();
    let p = tmp.path().join("foo.elf");
    fs::write(&p, vec![0u8; 100]).unwrap();
    cache_store(db.raw_conn(), "aa", &p, NOW).unwrap();

    let evicted = evict_lru(db.raw_conn(), 1_000_000).unwrap();
    assert!(evicted.is_empty(), "no evictions when cap exceeds total");
    // Confirm the row is still there.
    assert!(BuildCacheEntry::find(db.raw_conn(), "aa")
        .unwrap()
        .is_some());
}

#[test]
fn lookup_returns_miss_after_evict_lru_removed_the_row() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();
    let p = tmp.path().join("foo.elf");
    fs::write(&p, vec![0u8; 100]).unwrap();
    cache_store(db.raw_conn(), "aa", &p, NOW).unwrap();
    evict_lru(db.raw_conn(), 0).unwrap();

    assert_eq!(
        cache_lookup(db.raw_conn(), "aa", NOW + 1).unwrap(),
        CacheLookup::Miss
    );
}

#[test]
fn store_on_nonexistent_path_returns_io_error() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();
    let bogus = tmp.path().join("never-existed.elf");
    let err = cache_store(db.raw_conn(), "aa", &bogus, NOW).unwrap_err();
    assert!(
        matches!(err, paavo_core::CoreError::Io(_)),
        "expected CoreError::Io, got: {err:?}"
    );
}

#[test]
fn store_on_directory_path_returns_io_error() {
    let db = fresh_db();
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("not-a-file");
    fs::create_dir(&dir).unwrap();
    let err = cache_store(db.raw_conn(), "aa", &dir, NOW).unwrap_err();
    assert!(
        matches!(err, paavo_core::CoreError::Io(_)),
        "expected CoreError::Io for directory path, got: {err:?}"
    );
}
