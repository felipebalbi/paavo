//! axum router.

use crate::db::WebDb;
use axum::routing::get;
use axum::Router;

/// Build the router.
pub fn build_router(db: WebDb) -> Router {
    Router::new()
        .route("/", get(crate::pages::dashboard::render))
        .route("/jobs", get(crate::pages::jobs_list::render))
        .route("/jobs/:id", get(crate::pages::job_detail::render))
        .route("/boards", get(crate::pages::boards::render))
        .route("/schedule", get(crate::pages::schedule::render))
        .with_state(db)
}
