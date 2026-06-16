//! axum router.

use crate::db::WebDb;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

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
            (header::CONTENT_TYPE, HeaderValue::from_static("text/css; charset=utf-8")),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400, must-revalidate"),
            ),
        ],
        CSS,
    )
}

/// Build the router.
pub fn build_router(db: WebDb) -> Router {
    Router::new()
        .route("/", get(crate::pages::dashboard::render))
        .route("/jobs", get(crate::pages::jobs_list::render))
        .route("/jobs/:id", get(crate::pages::job_detail::render))
        .route("/boards", get(crate::pages::boards::render))
        .route("/schedule", get(crate::pages::schedule::render))
        .route("/static/style.css", get(serve_css))
        .with_state(db)
}
