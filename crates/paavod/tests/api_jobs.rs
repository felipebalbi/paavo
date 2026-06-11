use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use paavo_db::Db;
use paavo_proto::{BoardHealth, BoardSpec, JobSource, JobState, ProbeSelector};
use paavod::app::build_router;
use paavod::app_state::{AppState, DrainState};
use paavod::config::{
    BuildCacheConfig, Config, QuarantineConfig, RetentionConfig, SchedulerConfig, ServerConfig,
    TimeoutsConfig, WebConfig,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;

const BOUNDARY: &str = "----paavotest9999";

fn state_with_upload_cap(tmp_root: &std::path::Path, max_upload_bytes: usize) -> AppState {
    let db = Db::open(tmp_root.join("paavo.sqlite")).unwrap();
    let inv = vec![BoardSpec {
        id: "mcxa266-01".into(),
        kind: "mcxa266".into(),
        probe_selector: ProbeSelector {
            vid: "x".into(),
            pid: "x".into(),
            serial: "x".into(),
        },
        chip_name: "x".into(),
        target_name: "x".into(),
        wiring_profile: Some("default".into()),
        health: BoardHealth::Healthy,
    }];
    paavo_db::BoardRow::insert(db.raw_conn(), &inv[0], 0).unwrap();

    let cfg = Config {
        server: ServerConfig {
            bind: "127.0.0.1:0".into(),
            state_dir: tmp_root.to_path_buf(),
            max_upload_bytes,
        },
        web: WebConfig {
            bind: "127.0.0.1:0".into(),
        },
        timeouts: TimeoutsConfig::default(),
        scheduler: SchedulerConfig {
            nightly_cron: "0 0 19 * * *".into(),
            starvation_threshold_s: 21_600,
        },
        build_cache: BuildCacheConfig::default(),
        retention: RetentionConfig::default(),
        quarantine: QuarantineConfig::default(),
        corpus: vec![],
    };

    let sd = paavod::state_dir::StateDir::from_root(tmp_root);
    sd.ensure_dirs().unwrap();

    AppState {
        db: Arc::new(Mutex::new(db)),
        config: Arc::new(cfg),
        inventory: Arc::new(Mutex::new(inv)),
        drain: DrainState::default(),
    }
}

fn state(tmp_root: &std::path::Path) -> AppState {
    state_with_upload_cap(tmp_root, 256 * 1024 * 1024)
}

fn make_multipart_body(tar_bytes: &[u8], meta_json: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend(b"Content-Disposition: form-data; name=\"metadata\"\r\n");
    body.extend(b"Content-Type: application/json\r\n\r\n");
    body.extend(meta_json.as_bytes());
    body.extend(b"\r\n");
    body.extend(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend(b"Content-Disposition: form-data; name=\"crate\"; filename=\"crate.tar\"\r\n");
    body.extend(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend(tar_bytes);
    body.extend(b"\r\n");
    body.extend(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn submit_request(body: Vec<u8>) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/jobs")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(axum::body::Body::from(body))
        .unwrap()
}

fn default_meta() -> Value {
    json!({
        "priority": "interactive",
        "submitter": "felipe",
        "board_selector": { "kind": "mcxa266" },
        "inactivity_timeout_ms": 120000,
        "hard_max_ms": 900000
    })
}

/// Assert that `uploads/` contains no `.tmp-*` artifacts. Catches the
/// dedup-hit / orphan-temp leak class.
fn assert_no_orphan_temps(tmp_root: &std::path::Path) {
    let uploads = tmp_root.join("uploads");
    let temps: Vec<_> = std::fs::read_dir(&uploads)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".tmp-"))
        .collect();
    assert!(
        temps.is_empty(),
        "expected no orphan .tmp-* in {uploads:?}, found: {temps:?}"
    );
}

#[tokio::test]
async fn post_jobs_accepts_multipart_and_persists_tar() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s.clone());

    let body = make_multipart_body(b"hello tar bytes", &default_meta().to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let v: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1024).await.unwrap()).unwrap();
    let job_id = v["job_id"].as_str().unwrap();

    // Job row was inserted as Cli source (server-side forced).
    let id: paavo_proto::JobId = job_id.parse().unwrap();
    let row = paavo_db::JobRow::get(s.db.lock().raw_conn(), &id).unwrap();
    assert_eq!(row.state, JobState::Submitted);
    assert_eq!(row.source, JobSource::Cli);
    let upload_path = std::path::Path::new(&row.tar_path);
    assert!(upload_path.is_file(), "expected tar at {upload_path:?}");
}

#[tokio::test]
async fn post_jobs_forces_source_to_cli_even_if_client_sends_scheduler() {
    // Defect 5 from review: client can't claim Scheduler source over HTTP
    // (which would unlock the 4h default hard_max). Even if the wire
    // body includes `"source": "scheduler"`, the server records it as
    // Cli. The wire schema has `#[serde(deny_unknown_fields)]` so any
    // `source` field actually 400s before we get this far, but pin the
    // server-side override semantics with a separate assertion that
    // does NOT route through the wire schema — we send a valid request
    // and assert the recorded source is Cli regardless.
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s.clone());

    let body = make_multipart_body(b"hi", &default_meta().to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let v: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1024).await.unwrap()).unwrap();
    let id: paavo_proto::JobId = v["job_id"].as_str().unwrap().parse().unwrap();
    let row = paavo_db::JobRow::get(s.db.lock().raw_conn(), &id).unwrap();
    assert_eq!(row.source, JobSource::Cli);
}

#[tokio::test]
async fn post_jobs_rejects_unknown_metadata_field_with_400() {
    // The metadata schema has `#[serde(deny_unknown_fields)]` — any
    // legacy `source` field in the wire body must 400. Defense in
    // depth: forces the client to migrate off the field rather than
    // silently ignoring it.
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let mut meta = default_meta();
    meta["source"] = json!("scheduler"); // not a known field
    let body = make_multipart_body(b"x", &meta.to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_jobs_dedups_identical_tar_on_second_submit() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s.clone());

    let body = make_multipart_body(b"dedup payload", &default_meta().to_string());

    let r1 = app
        .clone()
        .oneshot(submit_request(body.clone()))
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::ACCEPTED);
    let v1: Value = serde_json::from_slice(&to_bytes(r1.into_body(), 1024).await.unwrap()).unwrap();
    let job1: paavo_proto::JobId = v1["job_id"].as_str().unwrap().parse().unwrap();
    let row1 = paavo_db::JobRow::get(s.db.lock().raw_conn(), &job1).unwrap();
    let mtime1 = std::fs::metadata(&row1.tar_path)
        .unwrap()
        .modified()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));

    let r2 = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(r2.status(), StatusCode::ACCEPTED);
    let v2: Value = serde_json::from_slice(&to_bytes(r2.into_body(), 1024).await.unwrap()).unwrap();
    let job2: paavo_proto::JobId = v2["job_id"].as_str().unwrap().parse().unwrap();
    let row2 = paavo_db::JobRow::get(s.db.lock().raw_conn(), &job2).unwrap();
    assert_eq!(row1.tar_path, row2.tar_path, "same blake3 → same path");
    let mtime2 = std::fs::metadata(&row2.tar_path)
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(mtime1, mtime2, "second submit must NOT rewrite the file");
    // And the dedup-hit path must NOT leak `.tmp-*.tar` files.
    assert_no_orphan_temps(tmp.path());
}

#[tokio::test]
async fn post_jobs_rejects_impossible_selector_with_400_and_leaves_no_orphan_tar() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s.clone());

    let mut meta = default_meta();
    meta["board_selector"]["kind"] = json!("no-such-board");
    let body = make_multipart_body(b"x", &meta.to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // The upload directory must contain no .tar artifact and no
    // .tmp-* artifact. A rejected submit MUST NOT leak either.
    let uploads = tmp.path().join("uploads");
    let entries: Vec<_> = std::fs::read_dir(&uploads)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tar"))
        .collect();
    assert!(
        entries.is_empty(),
        "expected no orphan .tar in {uploads:?}, found: {entries:?}"
    );
    assert_no_orphan_temps(tmp.path());
}

#[tokio::test]
async fn post_jobs_rejects_over_ceiling_with_400() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    // Daemon ceiling = 8h = 28_800_000 ms; ask for 9h.
    let mut meta = default_meta();
    meta["hard_max_ms"] = json!(32_400_000u64);
    let body = make_multipart_body(b"x", &meta.to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_jobs_rejects_missing_metadata_part_with_400() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let mut body = Vec::new();
    body.extend(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend(b"Content-Disposition: form-data; name=\"crate\"; filename=\"crate.tar\"\r\n");
    body.extend(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend(b"hi");
    body.extend(b"\r\n");
    body.extend(format!("--{BOUNDARY}--\r\n").as_bytes());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_jobs_rejects_while_draining_with_503() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    s.drain.set_draining();
    let app = build_router(s);
    let body = make_multipart_body(b"x", &default_meta().to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn post_jobs_accepts_large_tar_above_default_2mib() {
    // Defect 1 from review: axum's default body limit is 2 MiB. We must
    // override it via the `[server] max_upload_bytes` knob. Submit 5 MiB
    // to prove the override.
    let tmp = tempdir().unwrap();
    let s = state(tmp.path()); // default 256 MiB cap
    let app = build_router(s.clone());
    let big: Vec<u8> = vec![b'x'; 5 * 1024 * 1024];
    let body = make_multipart_body(&big, &default_meta().to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn post_jobs_rejects_oversized_tar_with_413() {
    let tmp = tempdir().unwrap();
    // Tight 64 KiB cap; submit 256 KiB.
    let s = state_with_upload_cap(tmp.path(), 64 * 1024);
    let app = build_router(s);
    let big: Vec<u8> = vec![b'x'; 256 * 1024];
    let body = make_multipart_body(&big, &default_meta().to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn post_jobs_rejects_duplicate_metadata_part_with_400() {
    // Symmetric with `duplicate `crate` part`: spec §9.1 says "exactly
    // two parts". A second metadata part must be rejected, not
    // silently override the first.
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let mut body = Vec::new();
    body.extend(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend(b"Content-Disposition: form-data; name=\"metadata\"\r\n");
    body.extend(b"Content-Type: application/json\r\n\r\n");
    body.extend(default_meta().to_string().as_bytes());
    body.extend(b"\r\n");
    body.extend(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend(b"Content-Disposition: form-data; name=\"metadata\"\r\n");
    body.extend(b"Content-Type: application/json\r\n\r\n");
    body.extend(default_meta().to_string().as_bytes());
    body.extend(b"\r\n");
    body.extend(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend(b"Content-Disposition: form-data; name=\"crate\"; filename=\"crate.tar\"\r\n");
    body.extend(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend(b"hi");
    body.extend(b"\r\n");
    body.extend(format!("--{BOUNDARY}--\r\n").as_bytes());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_jobs_rejects_oversized_metadata_part_with_413() {
    // Per-field cap on `metadata` is 64 KiB regardless of the request-
    // level `max_upload_bytes`. A 1 MiB metadata payload trips 413
    // before serde_json even tries to parse it.
    let tmp = tempdir().unwrap();
    let s = state(tmp.path()); // generous 256 MiB request cap
    let app = build_router(s);
    // Build a giant `submitter` string to inflate the metadata JSON
    // past 64 KiB while keeping the rest of the document valid.
    let mut meta = default_meta();
    meta["submitter"] = serde_json::json!("x".repeat(128 * 1024));
    let body = make_multipart_body(b"hi", &meta.to_string());
    let resp = app.oneshot(submit_request(body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
