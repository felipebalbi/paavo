use paavo_db::Db;
use tempfile::tempdir;

#[test]
fn open_runs_migrations_and_creates_tables() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let db = Db::open(&path).unwrap();

    let conn = db.raw_conn();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    // refinery_schema_history is the migrator's bookkeeping table — fine to
    // see, just filter it out.
    let user: Vec<&str> = tables
        .iter()
        .map(|s| s.as_str())
        .filter(|n| *n != "refinery_schema_history")
        .collect();

    assert_eq!(
        user,
        vec!["board", "build_cache", "job", "log_frame", "schedule"]
    );
}

#[test]
fn open_is_idempotent() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    {
        let _db = Db::open(&path).unwrap();
    }
    // Second open against same file must succeed (re-running migrations is a
    // no-op when they're already applied).
    let _db = Db::open(&path).unwrap();
}

#[test]
fn open_readonly_works_against_existing_db() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    {
        let _rw = Db::open(&path).unwrap();
    }
    let ro = Db::open_readonly(&path).unwrap();
    let count: i64 = ro
        .raw_conn()
        .query_row("SELECT COUNT(*) FROM board", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
