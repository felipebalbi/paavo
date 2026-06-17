//! Tests that the corpus enqueue helper inserts Scheduled jobs and
//! respects the spec §13 / §7.5 contract: explicit `kind` field on
//! `[[corpus]]`, `cargo_update` threaded into `JobRow.cargo_update_packages`,
//! `last_completed_at` only stamped on successful enqueues, schedule
//! row updated on every fire.
//!
//! Also covers startup seeding: `seed_schedule` registers the configured
//! `nightly_cron` in the `schedule` table at boot so paavo-web's
//! `/schedule` page shows it immediately, without waiting for the first
//! nightly fire.

use paavo_db::Db;
use paavo_proto::{BoardHealth, BoardSpec, JobSource, ProbeSelector};
use paavod::app_state::{AppState, DrainState};
use paavod::cancellation::CancellationRegistry;
use paavod::config::{
    BuildCacheConfig, Config, CorpusEntry, QuarantineConfig, RetentionConfig, SchedulerConfig,
    ServerConfig, TimeoutsConfig, WebConfig,
};
use paavod::job_logs::JobLogsBroker;
use parking_lot::Mutex;
use std::sync::Arc;

fn write_test_crate(dir: &std::path::Path, name: &str) {
    let crate_dir = dir.join(name);
    std::fs::create_dir_all(crate_dir.join("src")).unwrap();
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!("[package]\nname=\"{name}\"\nversion=\"0\"\n"),
    )
    .unwrap();
    std::fs::write(crate_dir.join("src/main.rs"), "fn main() {}").unwrap();
}

fn make_state(corpus: Vec<CorpusEntry>, state_root: &std::path::Path) -> AppState {
    let sd = paavod::state_dir::StateDir::from_root(state_root);
    sd.ensure_dirs().unwrap();
    let db = Db::open(&sd.sqlite_path).unwrap();
    paavo_db::BoardRow::insert(
        db.raw_conn(),
        &BoardSpec {
            id: "mcxa266-01".into(),
            kind: "mcxa266".into(),
            probe_selector: ProbeSelector {
                vid: "x".into(),
                pid: "x".into(),
                serial: "x".into(),
            },
            chip_name: "x".into(),
            target_name: "x".into(),
            wiring_profile: None,
            health: BoardHealth::Healthy,
        },
        0,
    )
    .unwrap();
    let inventory = paavo_db::BoardRow::list_all(db.raw_conn())
        .unwrap()
        .into_iter()
        .map(|r| r.spec)
        .collect::<Vec<_>>();
    let cfg = Arc::new(Config {
        server: ServerConfig {
            bind: "127.0.0.1:0".into(),
            state_dir: state_root.to_path_buf(),
            max_upload_bytes: 256 * 1024 * 1024,
        },
        web: WebConfig {
            bind: "127.0.0.1:0".into(),
        },
        timeouts: TimeoutsConfig::default(),
        scheduler: SchedulerConfig {
            nightly_cron: "0 0 19 * * *".into(),
            starvation_threshold_s: 21_600,
            max_concurrent_builds: 5,
        },
        build_cache: BuildCacheConfig::default(),
        retention: RetentionConfig::default(),
        quarantine: QuarantineConfig::default(),
        corpus,
    });
    AppState {
        db: Arc::new(Mutex::new(db)),
        config: cfg,
        inventory: Arc::new(Mutex::new(inventory)),
        drain: DrainState::default(),
        cancellation: CancellationRegistry::default(),
        build_cancel: paavod::cancellation::BuildCancelRegistry::default(),
        job_logs: JobLogsBroker::new(),
    }
}

#[tokio::test]
async fn seed_schedule_registers_nightly_row_from_config() {
    // The bug: until the nightly cron actually fires, the `schedule`
    // table is empty and paavo-web's /schedule page shows nothing.
    // `seed_schedule` must register the configured cron at startup.
    let tmp = tempfile::tempdir().unwrap();
    let state = make_state(vec![], &tmp.path().join("state"));

    // Precondition: no cron has fired, so the row does not yet exist.
    let before = paavo_db::ScheduleRow::get(state.db.lock().raw_conn(), "nightly").unwrap_err();
    assert!(
        matches!(before, paavo_db::DbError::NotFound { .. }),
        "schedule row should not exist before seeding"
    );

    paavod::cron::seed_schedule(&state).unwrap();

    let row = paavo_db::ScheduleRow::get(state.db.lock().raw_conn(), "nightly").unwrap();
    assert_eq!(row.cron, "0 0 19 * * *");
    assert!(row.enabled);
    assert!(
        row.last_triggered_at.is_none(),
        "a freshly seeded schedule has never fired"
    );
    assert!(
        row.last_completed_at.is_none(),
        "a freshly seeded schedule has never completed"
    );
}

#[tokio::test]
async fn seed_schedule_preserves_history_and_refreshes_cron() {
    // A restart re-seeds the schedule. The re-seed must REFRESH the
    // cron/enabled (so edits to nightly_cron land) but PRESERVE any
    // last_triggered_at / last_completed_at history from prior fires —
    // i.e. seed must pass NULL timestamps so upsert's COALESCE keeps
    // the existing values.
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    write_test_crate(&corpus_root, "test-a");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            kind: "mcxa266".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );

    // Simulate a nightly fire that stamps both trigger and completion.
    paavod::cron::__test_run_once(&state).await.unwrap();
    let fired = paavo_db::ScheduleRow::get(state.db.lock().raw_conn(), "nightly").unwrap();
    let triggered = fired.last_triggered_at.expect("fire stamps triggered_at");
    let completed = fired.last_completed_at.expect("fire stamps completed_at");

    // Restart-time re-seed must not clobber the stamps.
    paavod::cron::seed_schedule(&state).unwrap();

    let after = paavo_db::ScheduleRow::get(state.db.lock().raw_conn(), "nightly").unwrap();
    assert_eq!(
        after.last_triggered_at,
        Some(triggered),
        "re-seed must preserve last_triggered_at"
    );
    assert_eq!(
        after.last_completed_at,
        Some(completed),
        "re-seed must preserve last_completed_at"
    );
    assert_eq!(after.cron, "0 0 19 * * *");
    assert!(after.enabled);
}

#[tokio::test]
async fn corpus_entry_enqueues_one_job_per_crate_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    write_test_crate(&corpus_root, "test-a");
    write_test_crate(&corpus_root, "test-b");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            kind: "mcxa266".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );

    paavod::cron::__test_run_once(&state).await.unwrap();
    let rows = paavo_db::JobRow::list_by_state(
        state.db.lock().raw_conn(),
        paavo_proto::JobState::Submitted,
        50,
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.source == JobSource::Scheduler));
    assert!(rows.iter().all(|r| r.board_selector.kind == "mcxa266"));
    assert!(rows
        .iter()
        .all(|r| r.submitter.starts_with("nightly:test-corpus:")));
}

#[tokio::test]
async fn corpus_run_skips_non_dir_entries_and_dirs_without_cargo_toml() {
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    std::fs::create_dir_all(&corpus_root).unwrap();
    // A loose file (not a dir) — skipped.
    std::fs::write(corpus_root.join("notes.txt"), "ignored").unwrap();
    // A dir with no Cargo.toml — skipped.
    std::fs::create_dir_all(corpus_root.join("docs/chapters")).unwrap();
    // A real test crate — enqueued.
    write_test_crate(&corpus_root, "test-a");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            kind: "mcxa266".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );

    paavod::cron::__test_run_once(&state).await.unwrap();
    let rows = paavo_db::JobRow::list_by_state(
        state.db.lock().raw_conn(),
        paavo_proto::JobState::Submitted,
        50,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn corpus_run_no_ops_during_drain() {
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    write_test_crate(&corpus_root, "test-a");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            kind: "mcxa266".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );
    state.drain.set_draining();
    paavod::cron::__test_run_once(&state).await.unwrap();
    let rows = paavo_db::JobRow::list_by_state(
        state.db.lock().raw_conn(),
        paavo_proto::JobState::Submitted,
        50,
    )
    .unwrap();
    assert_eq!(rows.len(), 0, "drain must suppress new Scheduled enqueues");
    // And the schedule row must NOT have been touched — `/schedule`
    // should not show a stamped fire when the cron skipped.
    let sched_err = paavo_db::ScheduleRow::get(state.db.lock().raw_conn(), "nightly").unwrap_err();
    assert!(
        matches!(sched_err, paavo_db::DbError::NotFound { .. }),
        "schedule row should not exist after drain skip"
    );
}

#[tokio::test]
async fn corpus_run_updates_schedule_row_with_trigger_and_completion() {
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    write_test_crate(&corpus_root, "test-a");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            kind: "mcxa266".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );
    paavod::cron::__test_run_once(&state).await.unwrap();
    let row = paavo_db::ScheduleRow::get(state.db.lock().raw_conn(), "nightly").unwrap();
    assert!(row.enabled);
    let triggered = row
        .last_triggered_at
        .expect("last_triggered_at should be set");
    let completed = row
        .last_completed_at
        .expect("last_completed_at should be set after >=1 enqueue");
    assert!(
        completed >= triggered,
        "completed ({completed}) must be >= triggered ({triggered})"
    );
    assert_eq!(row.cron, "0 0 19 * * *");
}

#[tokio::test]
async fn corpus_run_advances_last_triggered_at_on_every_fire() {
    // M1 fix: ScheduleRow::upsert now COALESCEs last_triggered_at on
    // conflict so a second fire updates the column. Previously it was
    // frozen at the first fire's timestamp.
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    write_test_crate(&corpus_root, "test-a");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            kind: "mcxa266".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );
    paavod::cron::__test_run_once(&state).await.unwrap();
    let row1 = paavo_db::ScheduleRow::get(state.db.lock().raw_conn(), "nightly").unwrap();
    let triggered1 = row1.last_triggered_at.unwrap();
    // Wait long enough for chrono's millisecond clock to advance.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    paavod::cron::__test_run_once(&state).await.unwrap();
    let row2 = paavo_db::ScheduleRow::get(state.db.lock().raw_conn(), "nightly").unwrap();
    let triggered2 = row2.last_triggered_at.unwrap();
    assert!(
        triggered2 > triggered1,
        "second fire must advance last_triggered_at ({triggered1} -> {triggered2})"
    );
}

#[tokio::test]
async fn corpus_run_does_not_advance_completed_at_when_all_enqueues_fail() {
    // D1 fix: last_completed_at must remain NULL/stale when zero jobs
    // were successfully enqueued. Trigger the failure by targeting a
    // kind no board provides — every enqueue returns SelectorNeverMatches.
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("does-not-matter");
    write_test_crate(&corpus_root, "test-a");

    let state = make_state(
        vec![CorpusEntry {
            name: "ghost-corpus".into(),
            kind: "no-such-board-kind".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );
    paavod::cron::__test_run_once(&state).await.unwrap();
    let row = paavo_db::ScheduleRow::get(state.db.lock().raw_conn(), "nightly").unwrap();
    assert!(
        row.last_triggered_at.is_some(),
        "triggered_at must record the fire even when enqueues failed"
    );
    assert!(
        row.last_completed_at.is_none(),
        "completed_at must NOT advance when 0 jobs were enqueued"
    );
    // No jobs in the DB either.
    let rows = paavo_db::JobRow::list_by_state(
        state.db.lock().raw_conn(),
        paavo_proto::JobState::Submitted,
        50,
    )
    .unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn corpus_uses_explicit_kind_field_not_path_basename() {
    // B1 fix: the corpus path can be anything; the board kind comes
    // from the explicit `kind` field on `[[corpus]]`. Verify that a
    // path whose basename does NOT match any board kind still
    // produces successful enqueues when `kind` matches.
    let tmp = tempfile::tempdir().unwrap();
    // Path basename is "mcxa2xx" (a misspelling) but kind is
    // "mcxa266" (matches the seeded board).
    let corpus_root = tmp.path().join("mcxa2xx");
    write_test_crate(&corpus_root, "test-a");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            kind: "mcxa266".into(),
            path: corpus_root,
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );
    paavod::cron::__test_run_once(&state).await.unwrap();
    let rows = paavo_db::JobRow::list_by_state(
        state.db.lock().raw_conn(),
        paavo_proto::JobState::Submitted,
        50,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].board_selector.kind, "mcxa266");
}

#[tokio::test]
async fn corpus_threads_cargo_update_into_enqueued_job() {
    // B2 fix: the EnqueueRequest's cargo_update_packages is sourced
    // from CorpusEntry.cargo_update, persisted in the JobRow, and
    // therefore available to the dispatch loop when it constructs the
    // BuildPlan.
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    write_test_crate(&corpus_root, "test-a");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            kind: "mcxa266".into(),
            path: corpus_root,
            cargo_update: vec!["embassy-mcxa".into(), "embassy-executor".into()],
        }],
        &tmp.path().join("state"),
    );
    paavod::cron::__test_run_once(&state).await.unwrap();
    let rows = paavo_db::JobRow::list_by_state(
        state.db.lock().raw_conn(),
        paavo_proto::JobState::Submitted,
        50,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].cargo_update_packages,
        vec!["embassy-mcxa".to_string(), "embassy-executor".into()]
    );
}

#[tokio::test]
async fn corpus_dedups_identical_tar_contents() {
    // D2 + S4 fix: two crates with byte-identical contents must
    // produce one final <blake>.tar on disk (the temp from the second
    // crate is unlinked on dedup hit). No `.tmp-*` artifacts leak.
    let tmp = tempfile::tempdir().unwrap();
    let corpus_root = tmp.path().join("mcxa266");
    // Write the same crate twice into two different sibling directories
    // — they tar to identical bytes because tar archives the directory
    // name as part of the entry header. So we need IDENTICAL paths
    // post-tar, which means we use the SAME crate dir tree. To produce
    // two enqueue rows that share a blake3, we manually invoke
    // __test_run_once twice on the same single-crate corpus.
    write_test_crate(&corpus_root, "test-a");

    let state = make_state(
        vec![CorpusEntry {
            name: "test-corpus".into(),
            kind: "mcxa266".into(),
            path: corpus_root.clone(),
            cargo_update: vec![],
        }],
        &tmp.path().join("state"),
    );

    paavod::cron::__test_run_once(&state).await.unwrap();
    paavod::cron::__test_run_once(&state).await.unwrap();

    // Both rows should share the same tar_path.
    let rows = paavo_db::JobRow::list_by_state(
        state.db.lock().raw_conn(),
        paavo_proto::JobState::Submitted,
        50,
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].tar_path, rows[1].tar_path);
    assert_eq!(rows[0].tar_blake3, rows[1].tar_blake3);
    // Uploads dir should contain exactly one <blake>.tar (no orphans,
    // no `.tmp-*` artifacts).
    let uploads = tmp.path().join("state/uploads");
    let entries: Vec<_> = std::fs::read_dir(&uploads)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let tars: Vec<_> = entries.iter().filter(|n| n.ends_with(".tar")).collect();
    let temps: Vec<_> = entries.iter().filter(|n| n.starts_with(".tmp-")).collect();
    assert_eq!(
        tars.len(),
        1,
        "expected one persisted tar, got: {entries:?}"
    );
    assert!(
        temps.is_empty(),
        "expected no orphan .tmp-*, got: {entries:?}"
    );
}
