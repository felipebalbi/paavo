//! axum router.

use crate::db::WebDb;
use crate::index::LiveState;
use crate::proxy::{AppState, PaavodClient};
use axum::extract::FromRef;
use axum::routing::get;
use axum::Router;

// FromRef glue: each handler extracts only the slice of `AppState` it
// needs. `WebDb` and `PaavodClient` are `Arc`-backed; `LiveState` is a
// bundle of `Arc`s — all cheap to clone per request. The boards/
// schedules handlers extract the whole `State<AppState>` (they need DB
// rows *and* the live revision), which axum supplies via the blanket
// `FromRef<S> for S`.

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

impl FromRef<AppState> for LiveState {
    fn from_ref(s: &AppState) -> Self {
        s.live.clone()
    }
}

/// Build the router from a fully-constructed [`AppState`].
///
/// Used by both `paavo-web`'s `main` (real config + real reqwest
/// client) and integration tests (fake paavod URL plus an empty
/// in-memory sqlite). The single entry point keeps the route
/// definitions in one place — anything that wants to spin up the
/// router stays in sync.
///
/// Two kinds of route live here:
/// - the JSON/SSE API (`/api/*`) the WASM SPA fetches from, and
/// - a catch-all `fallback` that serves the embedded UI bundle (with a
///   `index.html` SPA fallback). Because the API routes are registered
///   first, they always win over the asset fallback.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/jobs", get(crate::api::jobs::list))
        .route("/api/jobs/:id", get(crate::api::jobs::get))
        .route("/api/jobs/:id/log", get(crate::api::jobs::log))
        .route("/api/jobs/:id/stream", get(crate::proxy::stream_job))
        .route("/api/boards", get(crate::api::boards::list))
        .route("/api/dashboard", get(crate::api::dashboard::get))
        .route("/api/schedules", get(crate::api::schedules::list))
        .route("/api/events", get(crate::api::events::events))
        .fallback(crate::embed::serve)
        .with_state(state)
}
