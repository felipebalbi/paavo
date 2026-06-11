//! axum app constructor.

use crate::app_state::AppState;
use crate::routes;
use axum::routing::{get, post};
use axum::Router;

/// Build the axum Router with all routes mounted.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(routes::health::health))
        .route("/ready", get(routes::health::ready))
        .route(
            "/jobs",
            post(routes::jobs::post_jobs).get(routes::jobs::list_jobs),
        )
        .route("/jobs/:id", get(routes::jobs::get_job))
        .route("/jobs/:id/cancel", post(routes::jobs::cancel_job))
        .route("/jobs/:id/stream", get(routes::jobs::stream_job))
        .route(
            "/boards",
            get(routes::boards::list_boards).post(routes::boards::add_board),
        )
        .route(
            "/boards/:id/quarantine",
            post(routes::boards::quarantine_board),
        )
        .route(
            "/boards/:id/unquarantine",
            post(routes::boards::unquarantine_board),
        )
        .with_state(state)
}
