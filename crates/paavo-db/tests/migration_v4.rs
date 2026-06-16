//! V4 must extend the job.state CHECK to allow 'awaiting_board' WITHOUT
//! losing log_frame rows to the ON DELETE CASCADE during the rebuild.
//!
//! The live `job` table is V1 + V2 (cargo_update_packages) + V3
//! (skip_cache), so the rebuild must preserve all 18 columns. refinery
//! runs migrations inside a transaction with foreign_keys=ON, so a naive
//! `DROP TABLE job` would cascade-delete every log_frame. This test pins
//! that V4 rebuilds both tables in dependency order and loses nothing.
use rusqlite::Connection;

#[test]
fn v4_preserves_data_and_allows_awaiting_board() {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();

    // Apply the full pre-V4 schema (V1 + V2 + V3).
    conn.execute_batch(include_str!("../migrations/V1__initial.sql")).unwrap();
    conn.execute_batch(include_str!("../migrations/V2__cargo_update_packages.sql")).unwrap();
    conn.execute_batch(include_str!("../migrations/V3__skip_cache.sql")).unwrap();

    // Seed a board + a running job + two log frames. cargo_update_packages
    // and skip_cache fall back to their column defaults.
    conn.execute_batch(
        "INSERT INTO board (id,kind,probe_selector,chip_name,target_name,health,consecutive_infra_failures,created_at)
         VALUES ('b','mcxa266','{}','c','t','healthy',0,0);
         INSERT INTO job (id,priority,submitter,source,board_selector,inactivity_timeout_ms,hard_max_ms,state,submitted_at,tar_blake3,tar_path)
         VALUES ('j1',0,'me','cli','{}',1,1,'running',0,'aaa','/x');
         INSERT INTO log_frame (job_id,seq,ts_us,level,message) VALUES ('j1',0,0,'info','hello');
         INSERT INTO log_frame (job_id,seq,ts_us,level,message) VALUES ('j1',1,0,'info','world');",
    ).unwrap();

    // Apply V4.
    conn.execute_batch(include_str!("../migrations/V4__awaiting_board.sql")).unwrap();

    // Job survived (state + the V2/V3 columns).
    let (state, pkgs, skip): (String, String, i64) = conn
        .query_row(
            "SELECT state, cargo_update_packages, skip_cache FROM job WHERE id='j1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, "running");
    assert_eq!(pkgs, "[]");
    assert_eq!(skip, 0);

    // Logs survived (no cascade loss).
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM log_frame WHERE job_id='j1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2, "log_frame rows must survive the job rebuild");

    // New state accepted by the extended CHECK.
    conn.execute(
        "INSERT INTO job (id,priority,submitter,source,board_selector,inactivity_timeout_ms,hard_max_ms,state,submitted_at,tar_blake3,tar_path)
         VALUES ('j2',0,'me','cli','{}',1,1,'awaiting_board',0,'bbb','/y')",
        [],
    ).unwrap();

    // FK still enforced after the rebuild.
    let bad = conn.execute(
        "INSERT INTO log_frame (job_id,seq,ts_us,level,message) VALUES ('nope',0,0,'info','x')",
        [],
    );
    assert!(bad.is_err(), "FK must still be enforced post-rebuild");

    // ON DELETE CASCADE still wired: deleting a job clears its frames.
    conn.execute("DELETE FROM job WHERE id='j1'", []).unwrap();
    let n2: i64 = conn
        .query_row("SELECT COUNT(*) FROM log_frame WHERE job_id='j1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n2, 0, "cascade delete must still fire after rebuild");
}
