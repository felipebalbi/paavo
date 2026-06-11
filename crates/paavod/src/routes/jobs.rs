//! /jobs/* handlers. Filled in by 4.2.b/c.

use crate::app_state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

/// Stub that respects spec §6.3 ("drain returns 503 for new jobs") even
/// though the real handler isn't here yet. Locks in the invariant.
fn drain_then(state: &AppState, what: &'static str) -> (StatusCode, &'static str) {
    if state.drain.is_draining() {
        (StatusCode::SERVICE_UNAVAILABLE, "paavod is draining")
    } else {
        (StatusCode::NOT_IMPLEMENTED, what)
    }
}

/// POST /jobs — placeholder. Returns 503 while draining (spec §6.3).
pub async fn post_jobs(State(s): State<AppState>) -> impl IntoResponse {
    drain_then(&s, "POST /jobs not yet wired")
}

/// GET /jobs — placeholder.
pub async fn list_jobs(_state: State<AppState>) -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, "GET /jobs not yet wired")
}

/// GET /jobs/:id — placeholder.
pub async fn get_job(_state: State<AppState>) -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, "GET /jobs/:id not yet wired")
}

/// POST /jobs/:id/cancel — placeholder.
pub async fn cancel_job(_state: State<AppState>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "POST /jobs/:id/cancel not yet wired",
    )
}

/// GET /jobs/:id/stream — placeholder.
pub async fn stream_job(_state: State<AppState>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "GET /jobs/:id/stream not yet wired",
    )
}
