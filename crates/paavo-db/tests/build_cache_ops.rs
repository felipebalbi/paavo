use chrono::Utc;
use paavo_db::{BuildCacheEntry, BuildCacheStats, Db};
use tempfile::tempdir;

fn fresh_db() -> Db {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();
    std::mem::forget(dir);
    db
}

#[test]
fn upsert_then_get_round_trips() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    let e = BuildCacheEntry {
        tar_blake3: "aaa".into(),
        elf_path: "/cache/aaa/foo.elf".into(),
        built_at: now,
        last_used_at: now,
        size_bytes: 1_000,
    };
    BuildCacheEntry::upsert(db.raw_conn(), &e).unwrap();
    let got = BuildCacheEntry::get(db.raw_conn(), "aaa").unwrap();
    assert_eq!(got, e);
}

#[test]
fn touch_last_used_advances_recency() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    BuildCacheEntry::upsert(
        db.raw_conn(),
        &BuildCacheEntry {
            tar_blake3: "aaa".into(),
            elf_path: "/c/foo.elf".into(),
            built_at: now,
            last_used_at: now,
            size_bytes: 100,
        },
    )
    .unwrap();
    BuildCacheEntry::touch_last_used(db.raw_conn(), "aaa", now + 1_000).unwrap();
    let e = BuildCacheEntry::get(db.raw_conn(), "aaa").unwrap();
    assert_eq!(e.last_used_at, now + 1_000);
    assert_eq!(e.built_at, now);
}

#[test]
fn stats_reports_total_size_and_count() {
    let db = fresh_db();
    let now = Utc::now().timestamp_millis();
    for (k, s) in [("a", 100u64), ("b", 250), ("c", 75)] {
        BuildCacheEntry::upsert(
            db.raw_conn(),
            &BuildCacheEntry {
                tar_blake3: k.into(),
                elf_path: format!("/c/{k}.elf"),
                built_at: now,
                last_used_at: now,
                size_bytes: s,
            },
        )
        .unwrap();
    }
    let st = BuildCacheEntry::stats(db.raw_conn()).unwrap();
    assert_eq!(
        st,
        BuildCacheStats {
            total_bytes: 425,
            count: 3
        }
    );
}

#[test]
fn evict_until_under_drops_least_recently_used_first() {
    let db = fresh_db();
    let t = Utc::now().timestamp_millis();
    // Insert three entries with increasing recency.
    BuildCacheEntry::upsert(
        db.raw_conn(),
        &BuildCacheEntry {
            tar_blake3: "oldest".into(),
            elf_path: "/c/o.elf".into(),
            built_at: t,
            last_used_at: t,
            size_bytes: 100,
        },
    )
    .unwrap();
    BuildCacheEntry::upsert(
        db.raw_conn(),
        &BuildCacheEntry {
            tar_blake3: "middle".into(),
            elf_path: "/c/m.elf".into(),
            built_at: t,
            last_used_at: t + 100,
            size_bytes: 100,
        },
    )
    .unwrap();
    BuildCacheEntry::upsert(
        db.raw_conn(),
        &BuildCacheEntry {
            tar_blake3: "newest".into(),
            elf_path: "/c/n.elf".into(),
            built_at: t,
            last_used_at: t + 200,
            size_bytes: 100,
        },
    )
    .unwrap();

    // Total = 300; cap to 150. Expect 'oldest' and 'middle' dropped.
    let evicted = BuildCacheEntry::evict_until_under(db.raw_conn(), 150).unwrap();
    assert_eq!(
        evicted
            .iter()
            .map(|e| e.tar_blake3.as_str())
            .collect::<Vec<_>>(),
        vec!["oldest", "middle"]
    );
    let st = BuildCacheEntry::stats(db.raw_conn()).unwrap();
    assert_eq!(st.total_bytes, 100);
    assert_eq!(st.count, 1);
}

#[test]
fn evict_until_under_noop_when_already_under_cap() {
    let db = fresh_db();
    let t = Utc::now().timestamp_millis();
    BuildCacheEntry::upsert(
        db.raw_conn(),
        &BuildCacheEntry {
            tar_blake3: "only".into(),
            elf_path: "/c/o.elf".into(),
            built_at: t,
            last_used_at: t,
            size_bytes: 50,
        },
    )
    .unwrap();
    let evicted = BuildCacheEntry::evict_until_under(db.raw_conn(), 100).unwrap();
    assert!(evicted.is_empty());
}
