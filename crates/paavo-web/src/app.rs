//! axum router.

use crate::db::WebDb;
use crate::proxy::{AppState, PaavodClient};
use axum::extract::FromRef;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

// FromRef glue: page handlers keep their `State<WebDb>` extractor
// untouched (no per-page edit needed). The proxy handler extracts
// `State<AppState>` directly. axum `from_ref`-derives the substate
// for each request from the parent state at extract time; cloning
// `WebDb` is just `Arc::clone` and `PaavodClient` is the same.

impl FromRef<AppState> for WebDb {
    fn from_ref(s: &AppState) -> Self {
        s.db.clone()
    }
}

impl FromRef<AppState> for PaavodClient {
    fn from_ref(s: &AppState) -> Self {
        s.paavod.clone()
    }
}

/// `/static/style.css` — serves the baked-in ef-cyprus + ef-symbiosis
/// stylesheet. The bytes are pulled in at compile time via
/// `include_str!` so paavo-web stays a single binary deploy and the
/// CSS is byte-identical to whatever was committed alongside the code.
///
/// Two cache headers:
/// - `cache-control: public, max-age=86400, must-revalidate` — tell
///   the browser it's safe to cache for a day; `must-revalidate`
///   forces a conditional GET on the next visit so we don't serve a
///   stale CSS for weeks if a user keeps the tab open.
/// - The HTML shell appends `?v={CARGO_PKG_VERSION}` to the link so a
///   release-built paavo-web invalidates browser caches mechanically
///   even if max-age would otherwise miss.
async fn serve_css() -> impl IntoResponse {
    const CSS: &str = include_str!("assets/style.css");
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/css; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400, must-revalidate"),
            ),
        ],
        CSS,
    )
}

/// `/static/live-log.js` — serves the EventSource consumer that
/// drives the live log pane on `/jobs/:id`. Same caching contract
/// as `/static/style.css`: bake at compile time, year-long cache,
/// must-revalidate, version-busted via `?v={CARGO_PKG_VERSION}` on
/// the `<script src=...>` link rendered by `pages::job_detail`.
async fn serve_live_log_js() -> impl IntoResponse {
    const JS: &str = include_str!("assets/live-log.js");
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/javascript; charset=utf-8"),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400, must-revalidate"),
            ),
        ],
        JS,
    )
}

/// Build the router from a fully-constructed [`AppState`].
///
/// Used by both `paavo-web`'s `main` (real config + real reqwest
/// client) and integration tests (fake paavod URL plus an empty
/// in-memory sqlite). The single entry point keeps the route
/// definitions in one place — anything that wants to spin up the
/// router stays in sync.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(crate::pages::dashboard::render))
        .route("/jobs", get(crate::pages::jobs_list::render))
        .route("/jobs/:id", get(crate::pages::job_detail::render))
        .route("/boards", get(crate::pages::boards::render))
        .route("/schedule", get(crate::pages::schedule::render))
        .route("/static/style.css", get(serve_css))
        .route("/static/live-log.js", get(serve_live_log_js))
        .route("/api/jobs/:id/stream", get(crate::proxy::stream_job))
        .with_state(state)
}
