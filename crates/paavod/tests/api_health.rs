use axum::body::to_bytes;
use axum::http::Request;
use paavo_db::Db;
use paavod::app::build_router;
use paavod::app_state::{AppState, DrainState};
use paavod::config::{
    BuildCacheConfig, Config, QuarantineConfig, RetentionConfig, SchedulerConfig, ServerConfig,
    TimeoutsConfig, WebConfig,
};
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;

fn make_state() -> AppState {
    // Cross-platform: derive every path from the same tempdir so the
    // test never embeds a Unix-only `/tmp/...` literal. The dir is
    // leaked per workspace convention — see
    // `crates/paavo-core/tests/common/mod.rs::fresh_db`.
    let dir = tempdir().unwrap();
    let state_dir = dir.path().to_path_buf();
    let db = Db::open(state_dir.join("paavo.sqlite")).unwrap();
    std::mem::forget(dir);
    let cfg = Config {
        server: ServerConfig {
            bind: "127.0.0.1:0".into(),
            state_dir,
            max_upload_bytes: 256 * 1024 * 1024,
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
    AppState {
        db: Arc::new(Mutex::new(db)),
        config: Arc::new(cfg),
        inventory: Arc::new(Mutex::new(vec![])),
        drain: DrainState::default(),
        job_logs: paavod::job_logs::JobLogsBroker::new(),
        cancellation: paavod::cancellation::CancellationRegistry::default(),
    }
}

async fn get(state: AppState, uri: &str) -> (axum::http::StatusCode, Value) {
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 2048).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    (status, body)
}

#[tokio::test]
async fn health_is_200_with_body() {
    let (status, body) = get(make_state(), "/health").await;
    assert_eq!(status, 200);
    assert_eq!(body["service"], "paavod");
    assert_eq!(body["ready"], true);
}

#[tokio::test]
async fn health_stays_200_while_draining() {
    // Spec §9.5: `/health` is liveness — must return 200 even while
    // draining, otherwise systemd / k8s probes kill us mid-drain. The
    // body MUST report `ready: false` so monitoring can still observe
    // the drain.
    let state = make_state();
    state.drain.set_draining();
    let (status, body) = get(state, "/health").await;
    assert_eq!(status, 200);
    assert_eq!(body["service"], "paavod");
    assert_eq!(body["ready"], false);
}

#[tokio::test]
async fn ready_is_200_when_not_draining() {
    let (status, body) = get(make_state(), "/ready").await;
    assert_eq!(status, 200);
    assert_eq!(body["ready"], true);
}

#[tokio::test]
async fn ready_flips_to_503_when_draining() {
    let state = make_state();
    state.drain.set_draining();
    let (status, body) = get(state, "/ready").await;
    assert_eq!(status, 503);
    assert_eq!(body["service"], "paavod");
    assert_eq!(body["ready"], false);
}
