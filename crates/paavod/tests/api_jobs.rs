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
            interface: None,
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
            max_concurrent_builds: 5,
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
        job_logs: paavod::job_logs::JobLogsBroker::new(),
        cancellation: paavod::cancellation::CancellationRegistry::default(),
        build_cancel: paavod::cancellation::BuildCancelRegistry::default(),
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
async fn ten_submits_are_all_accepted() {
    // INV-1: the build cap never gates acceptance. Ten back-to-back
    // submits each get a 202 + a distinct persisted job row.
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let mut ids = std::collections::HashSet::new();
    for i in 0..10 {
        let app = build_router(s.clone());
        let body = make_multipart_body(format!("tar-{i}").as_bytes(), &default_meta().to_string());
        let resp = app.oneshot(submit_request(body)).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::ACCEPTED,
            "submit {i} must be accepted"
        );
        let v: Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), 1024).await.unwrap()).unwrap();
        ids.insert(v["job_id"].as_str().unwrap().to_string());
    }
    assert_eq!(ids.len(), 10, "10 distinct job ids");
    let rows =
        paavo_db::JobRow::list_by_state(s.db.lock().raw_conn(), JobState::Submitted, 500).unwrap();
    assert_eq!(rows.len(), 10, "all 10 jobs persisted as Submitted");
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
async fn get_job_returns_row() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    // Insert directly.
    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    let app = build_router(s);
    let req = Request::builder()
        .uri(format!("/jobs/{id}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["id"], id.to_string());
    assert_eq!(v["state"], "submitted");
}

#[tokio::test]
async fn cancel_submitted_job_returns_204() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    let app = build_router(s.clone());
    let req = Request::builder()
        .method("POST")
        .uri(format!("/jobs/{id}/cancel"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 204);
    let row = paavo_db::JobRow::get(s.db.lock().raw_conn(), &id).unwrap();
    assert_eq!(row.state, paavo_proto::JobState::Aborted);
}

#[tokio::test]
async fn cancel_awaiting_board_job_returns_204() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    // Drive to AwaitingBoard via the two-stage transitions (no board).
    paavo_db::JobRow::transition_submitted_to_building(s.db.lock().raw_conn(), &id, 1).unwrap();
    paavo_db::JobRow::transition_building_to_awaiting_board(s.db.lock().raw_conn(), &id, "/e.elf")
        .unwrap();

    let app = build_router(s.clone());
    let req = Request::builder()
        .method("POST")
        .uri(format!("/jobs/{id}/cancel"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 204);
    let row = paavo_db::JobRow::get(s.db.lock().raw_conn(), &id).unwrap();
    assert_eq!(row.state, paavo_proto::JobState::Aborted);
}

#[tokio::test]
async fn list_jobs_filters_by_state() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    for _ in 0..3 {
        let id = paavo_proto::JobId::new();
        paavo_db::JobRow::insert(
            s.db.lock().raw_conn(),
            &paavo_db::NewJob {
                id,
                priority: paavo_proto::Priority::Interactive,
                submitter: "x".into(),
                source: paavo_proto::JobSource::Cli,
                board_selector: paavo_proto::BoardSelector {
                    kind: "mcxa266".into(),
                    instance: None,
                    wiring_profile: None,
                },
                inactivity_timeout_ms: 120_000,
                hard_max_ms: 900_000,
                tar_blake3: "x".into(),
                tar_path: "/tmp/x.tar".into(),
                cargo_update_packages: vec![],
                skip_cache: false,
            },
            0,
        )
        .unwrap();
    }
    let app = build_router(s);
    let req = Request::builder()
        .uri("/jobs?state=submitted&limit=2")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn get_unknown_job_returns_404() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let req = Request::builder()
        .uri(format!("/jobs/{}", paavo_proto::JobId::new()))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn cancel_unknown_job_returns_404() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let req = Request::builder()
        .method("POST")
        .uri(format!("/jobs/{}/cancel", paavo_proto::JobId::new()))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
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

#[tokio::test]
async fn list_jobs_rejects_unknown_state_with_400() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let req = Request::builder()
        .uri("/jobs?state=garbage")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_jobs_rejects_unparseable_limit_with_400() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let req = Request::builder()
        .uri("/jobs?limit=fifty")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_jobs_rejects_out_of_range_limit_with_400() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    for bad in ["0", "501", "999999"] {
        let req = Request::builder()
            .uri(format!("/jobs?limit={bad}"))
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "limit={bad}");
    }
}

#[tokio::test]
async fn get_job_rejects_invalid_id_with_400() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let req = Request::builder()
        .uri("/jobs/not-a-ulid")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn cancel_job_rejects_invalid_id_with_400() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let req = Request::builder()
        .method("POST")
        .uri("/jobs/not-a-ulid/cancel")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn cancel_terminal_job_returns_409() {
    // Pins spec §5.4: terminal jobs are not cancellable. First cancel
    // succeeds (Submitted → Aborted), second returns 409 because the
    // row is already terminal AND the cancellation registry has no
    // entry to signal (so the registry-fallback path also fails).
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    let app = build_router(s.clone());
    // First cancel: 204.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/jobs/{id}/cancel"))
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), 204);
    // Second cancel on the now-terminal row: 409.
    let req = Request::builder()
        .method("POST")
        .uri(format!("/jobs/{id}/cancel"))
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(app.oneshot(req).await.unwrap().status(), 409);
}

#[tokio::test]
async fn get_job_view_omits_tar_path_and_elf_path() {
    // Pins the wire-shape contract: server-local filesystem paths must
    // not leak through GET /jobs/:id.
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "deadbeef".into(),
            tar_path: "/var/lib/paavo/uploads/deadbeef.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    let app = build_router(s);
    let req = Request::builder()
        .uri(format!("/jobs/{id}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(v.get("tar_path").is_none(), "tar_path leaked: {v}");
    assert!(v.get("elf_path").is_none(), "elf_path leaked: {v}");
    // blake3 IS exposed (content-addressed, useful for build-cache debugging).
    assert_eq!(v["tar_blake3"], "deadbeef");
}

#[tokio::test]
async fn stream_terminal_returns_historical_plus_outcome() {
    use paavo_db::LogFrameDb;
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());

    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    paavo_proto::LogFrame::append_batch(
        s.db.lock().raw_conn(),
        &id,
        &[paavo_proto::LogFrame {
            seq: 0,
            ts_us: 0,
            level: paavo_proto::LogLevel::Info,
            target: None,
            message: "hi".into(),
        }],
    )
    .unwrap();
    paavo_db::JobRow::finalize(
        s.db.lock().raw_conn(),
        &id,
        &paavo_db::OutcomeRecord {
            state: paavo_proto::JobState::Passed,
            outcome: paavo_proto::JobOutcome::Passed,
            finished_at_ms: 1,
        },
    )
    .unwrap();

    let app = build_router(s);
    let req = Request::builder()
        .uri(format!("/jobs/{id}/stream"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/x-ndjson"
    );
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let lines: Vec<&str> = text.split('\n').filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 2, "expected 2 NDJSON lines, got: {text}");
    let frame_line: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(frame_line["type"], "frame");
    assert_eq!(frame_line["frame"]["message"], "hi");
    let term_line: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(term_line["type"], "terminal");
    assert_eq!(term_line["outcome"], "passed");
}

#[tokio::test]
async fn stream_unknown_job_returns_404() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let req = Request::builder()
        .uri(format!("/jobs/{}/stream", paavo_proto::JobId::new()))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn stream_rejects_invalid_id_with_400() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let app = build_router(s);
    let req = Request::builder()
        .uri("/jobs/not-a-ulid/stream")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn stream_live_emits_published_frame_then_terminal() {
    // Insert a Running job (no historical frames yet). Spawn a task
    // that publishes a frame + terminal after a short delay so the
    // handler subscribes first.
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    paavo_db::JobRow::transition_to_building(s.db.lock().raw_conn(), &id, "mcxa266-01", 1).unwrap();
    paavo_db::JobRow::transition_to_running(s.db.lock().raw_conn(), &id, "/elf").unwrap();

    let broker = s.job_logs.clone();
    tokio::spawn(async move {
        // Poll until the handler has subscribed before publishing — the
        // alternative is a fixed sleep, which is flaky on loaded CI.
        for _ in 0..200 {
            if broker.active_channels() > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        broker.publish(
            id,
            paavod::job_logs::LiveEvent::Frame(paavo_proto::LogFrame {
                seq: 0,
                ts_us: 0,
                level: paavo_proto::LogLevel::Info,
                target: None,
                message: "live".into(),
            }),
        );
        broker.publish(
            id,
            paavod::job_logs::LiveEvent::Terminal(paavo_proto::JobOutcome::Passed),
        );
    });

    let app = build_router(s);
    let req = Request::builder()
        .uri(format!("/jobs/{id}/stream"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let lines: Vec<&str> = text.split('\n').filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 2, "expected 2 NDJSON lines, got: {text}");
    let frame_line: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(frame_line["type"], "frame");
    assert_eq!(frame_line["frame"]["message"], "live");
    let term_line: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(term_line["type"], "terminal");
}

#[tokio::test]
async fn stream_terminal_does_not_leak_subscriber_channels() {
    // Pins the no-leak invariant: a /stream request for an already-
    // terminal job must NOT create a Sender in the broker (the
    // worker won't come back to call finalize()).
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());

    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    paavo_db::JobRow::finalize(
        s.db.lock().raw_conn(),
        &id,
        &paavo_db::OutcomeRecord {
            state: paavo_proto::JobState::Passed,
            outcome: paavo_proto::JobOutcome::Passed,
            finished_at_ms: 1,
        },
    )
    .unwrap();

    assert_eq!(s.job_logs.active_channels(), 0);
    let app = build_router(s.clone());
    let req = Request::builder()
        .uri(format!("/jobs/{id}/stream"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let _ = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    assert_eq!(
        s.job_logs.active_channels(),
        0,
        "terminal-job stream must not leave a Sender behind"
    );
}

#[tokio::test]
async fn stream_unknown_job_does_not_leak_subscriber_channels() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    assert_eq!(s.job_logs.active_channels(), 0);
    let app = build_router(s.clone());
    let req = Request::builder()
        .uri(format!("/jobs/{}/stream", paavo_proto::JobId::new()))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(
        s.job_logs.active_channels(),
        0,
        "404 stream must not leave a Sender behind"
    );
}

#[tokio::test]
async fn stream_returns_500_when_terminal_outcome_is_null() {
    // Pins the corrupted-DB defensive close: terminal state + NULL
    // outcome should surface as 500, NOT a fabricated Aborted{User}.
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    // Force a corrupt state: write state='passed' but leave
    // outcome_detail NULL by bypassing JobRow::finalize.
    s.db.lock()
        .raw_conn()
        .execute(
            "UPDATE job SET state = 'passed', outcome_detail = NULL,
             finished_at = 1 WHERE id = ?1",
            rusqlite::params![id.to_string()],
        )
        .unwrap();
    let app = build_router(s);
    let req = Request::builder()
        .uri(format!("/jobs/{id}/stream"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 500);
}

#[tokio::test]
async fn stream_pages_historical_above_chunk_size() {
    // Pins that the historical fetch is NOT capped at 10k (or any
    // single chunk size). Inserts 1500 frames (above the 1000-frame
    // page) and asserts they all appear in the stream.
    use paavo_db::LogFrameDb;
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());

    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    let frames: Vec<paavo_proto::LogFrame> = (0..1500u64)
        .map(|i| paavo_proto::LogFrame {
            seq: i,
            ts_us: i,
            level: paavo_proto::LogLevel::Info,
            target: None,
            message: format!("frame-{i}"),
        })
        .collect();
    paavo_proto::LogFrame::append_batch(s.db.lock().raw_conn(), &id, &frames).unwrap();
    paavo_db::JobRow::finalize(
        s.db.lock().raw_conn(),
        &id,
        &paavo_db::OutcomeRecord {
            state: paavo_proto::JobState::Passed,
            outcome: paavo_proto::JobOutcome::Passed,
            finished_at_ms: 2,
        },
    )
    .unwrap();

    let app = build_router(s);
    let req = Request::builder()
        .uri(format!("/jobs/{id}/stream"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    // Allow up to ~2 MiB for the body (1500 frames * ~50 bytes each).
    let bytes = to_bytes(resp.into_body(), 4 * 1024 * 1024).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let lines: Vec<&str> = text.split('\n').filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 1501, "1500 frames + 1 terminal");
    // First and last frames should be present in order.
    let first_frame: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first_frame["frame"]["message"], "frame-0");
    let last_frame: Value = serde_json::from_str(lines[1499]).unwrap();
    assert_eq!(last_frame["frame"]["message"], "frame-1499");
    let term: Value = serde_json::from_str(lines[1500]).unwrap();
    assert_eq!(term["type"], "terminal");
}

#[tokio::test]
async fn cancel_running_job_with_registered_signal_returns_204() {
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    paavo_db::JobRow::transition_to_building(s.db.lock().raw_conn(), &id, "mcxa266-01", 1).unwrap();
    paavo_db::JobRow::transition_to_running(s.db.lock().raw_conn(), &id, "/elf").unwrap();

    s.cancellation.register(id);
    let rx = s.cancellation.take_receiver(&id).unwrap();

    let app = build_router(s);
    let req = Request::builder()
        .method("POST")
        .uri(format!("/jobs/{id}/cancel"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 204);
    assert_eq!(rx.recv().unwrap(), paavo_runner::RunCommand::Cancel);
}

#[tokio::test]
async fn cancel_running_job_without_registered_signal_returns_409() {
    // Job is Running in the DB but the registry has no sender (worker
    // died, registry entry already unregistered, dispatch loop not
    // running yet, etc.). The cancel handler falls through to 409.
    let tmp = tempdir().unwrap();
    let s = state(tmp.path());
    let id = paavo_proto::JobId::new();
    paavo_db::JobRow::insert(
        s.db.lock().raw_conn(),
        &paavo_db::NewJob {
            id,
            priority: paavo_proto::Priority::Interactive,
            submitter: "x".into(),
            source: paavo_proto::JobSource::Cli,
            board_selector: paavo_proto::BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "x".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        },
        0,
    )
    .unwrap();
    paavo_db::JobRow::transition_to_building(s.db.lock().raw_conn(), &id, "mcxa266-01", 1).unwrap();
    paavo_db::JobRow::transition_to_running(s.db.lock().raw_conn(), &id, "/elf").unwrap();

    let app = build_router(s);
    let req = Request::builder()
        .method("POST")
        .uri(format!("/jobs/{id}/cancel"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 409);
}
