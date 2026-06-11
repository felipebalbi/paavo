use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use paavo_db::Db;
use paavo_proto::{BoardHealth, BoardSpec, ProbeSelector};
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

fn state() -> AppState {
    // Cross-platform: derive every path from the same tempdir; leak the
    // dir per the workspace convention in
    // `crates/paavo-core/tests/common/mod.rs::fresh_db`.
    let dir = tempdir().unwrap();
    let state_dir = dir.path().to_path_buf();
    let db = Db::open(state_dir.join("paavo.sqlite")).unwrap();
    std::mem::forget(dir);
    let cfg = Config {
        server: ServerConfig {
            bind: "127.0.0.1:0".into(),
            state_dir,
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
    }
}

fn sample_board_json() -> Value {
    json!({
        "id": "mcxa266-01",
        "kind": "mcxa266",
        "probe_selector": { "vid": "1366", "pid": "1015", "serial": "ABC" },
        "chip_name": "MCXA266VFL",
        "target_name": "frdm-mcx-a266",
        "wiring_profile": "default",
        "health": "healthy"
    })
}

async fn post_json(
    app: axum::Router,
    uri: &str,
    body: Value,
) -> axum::http::Response<axum::body::Body> {
    let bytes = serde_json::to_vec(&body).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(bytes))
        .unwrap();
    app.oneshot(req).await.unwrap()
}

async fn post_empty(app: axum::Router, uri: &str) -> axum::http::Response<axum::body::Body> {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    app.oneshot(req).await.unwrap()
}

async fn read_json(resp: axum::http::Response<axum::body::Body>) -> Value {
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn post_boards_then_get_boards_returns_full_view() {
    let s = state();
    let app = build_router(s.clone());

    let resp = post_json(app.clone(), "/boards", sample_board_json()).await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = Request::builder()
        .uri("/boards")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v = read_json(resp).await;
    assert_eq!(v.as_array().unwrap().len(), 1);
    // GET /boards returns BoardView, not BoardSpec. The view exposes
    // the operational fields §9.4 promises.
    assert_eq!(v[0]["id"], "mcxa266-01");
    assert_eq!(v[0]["consecutive_infra_failures"], 0);
    assert!(v[0]["created_at"].as_i64().unwrap() > 0);
    // No quarantine, so no reason field.
    assert!(v[0].get("quarantine_reason").is_none());
    // last_used_at is None on a freshly added board.
    assert!(v[0].get("last_used_at").is_none());

    let inv = s.inventory_snapshot();
    assert_eq!(inv.len(), 1);
    assert_eq!(inv[0].id, "mcxa266-01");
}

#[tokio::test]
async fn get_boards_orders_by_id_ascending() {
    // Locks in the contract that paavo-db's `list_all` ORDER BY id ASC
    // is preserved through the HTTP layer. paavo-cli renders fleets
    // in this order.
    let s = state();
    let app = build_router(s.clone());

    let mut a = sample_board_json();
    a["id"] = json!("mcxa266-02");
    a["probe_selector"]["serial"] = json!("BBB");
    let mut b = sample_board_json();
    b["id"] = json!("mcxa266-01");
    b["probe_selector"]["serial"] = json!("AAA");
    assert_eq!(
        post_json(app.clone(), "/boards", a).await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        post_json(app.clone(), "/boards", b).await.status(),
        StatusCode::CREATED
    );

    let req = Request::builder()
        .uri("/boards")
        .body(axum::body::Body::empty())
        .unwrap();
    let v = read_json(app.oneshot(req).await.unwrap()).await;
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["id"], "mcxa266-01");
    assert_eq!(arr[1]["id"], "mcxa266-02");
}

#[tokio::test]
async fn post_boards_rejects_duplicate_id_with_409() {
    let s = state();
    let app = build_router(s.clone());

    let resp = post_json(app.clone(), "/boards", sample_board_json()).await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = post_json(app, "/boards", sample_board_json()).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn post_boards_rejects_non_healthy_with_400() {
    // §9.4: initial registration must be `Healthy`; the quarantine flow
    // requires a `reason`. Accepting `health: "quarantined"` here would
    // let a client create a quarantined board with `quarantine_reason
    // = NULL`, violating the data invariant.
    let s = state();
    let app = build_router(s.clone());

    let mut body = sample_board_json();
    body["health"] = json!("quarantined");
    let resp = post_json(app, "/boards", body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn quarantine_unknown_board_returns_404() {
    let s = state();
    let app = build_router(s.clone());
    let resp = post_json(app, "/boards/ghost/quarantine", json!({"reason": "x"})).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unquarantine_unknown_board_returns_404() {
    let s = state();
    let app = build_router(s.clone());
    let resp = post_empty(app, "/boards/ghost/unquarantine").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn quarantine_rejects_empty_reason_with_400() {
    let s = state();
    let app = build_router(s.clone());
    assert_eq!(
        post_json(app.clone(), "/boards", sample_board_json())
            .await
            .status(),
        StatusCode::CREATED,
    );
    let resp = post_json(
        app,
        "/boards/mcxa266-01/quarantine",
        json!({"reason": "   "}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn quarantine_and_unquarantine_flip_health_and_cache() {
    let s = state();
    // Seed directly via db so the test pins the cache-refresh contract
    // independently of `add_board`.
    paavo_db::BoardRow::insert(
        s.db.lock().raw_conn(),
        &BoardSpec {
            id: "b".into(),
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
    *s.inventory.lock() = paavo_db::BoardRow::list_all(s.db.lock().raw_conn())
        .unwrap()
        .into_iter()
        .map(|r| r.spec)
        .collect();

    let app = build_router(s.clone());

    let resp = post_json(
        app.clone(),
        "/boards/b/quarantine",
        json!({"reason": "broken header"}),
    )
    .await;
    assert_eq!(resp.status(), 204);
    let row = paavo_db::BoardRow::get(s.db.lock().raw_conn(), "b").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Quarantined);
    assert_eq!(row.quarantine_reason.as_deref(), Some("broken header"));
    assert_eq!(s.inventory_snapshot()[0].health, BoardHealth::Quarantined);

    let resp = post_empty(app, "/boards/b/unquarantine").await;
    assert_eq!(resp.status(), 204);
    let row = paavo_db::BoardRow::get(s.db.lock().raw_conn(), "b").unwrap();
    assert_eq!(row.spec.health, BoardHealth::Healthy);
    assert!(row.quarantine_reason.is_none());
    assert_eq!(s.inventory_snapshot()[0].health, BoardHealth::Healthy);
}
