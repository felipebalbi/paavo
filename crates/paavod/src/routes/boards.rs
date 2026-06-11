//! /boards/* handlers. Filled in by 4.2.c.

use crate::app_state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

/// GET /boards — placeholder.
pub async fn list_boards(_state: State<AppState>) -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, "GET /boards not yet wired")
}

/// POST /boards — placeholder.
pub async fn add_board(_state: State<AppState>) -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, "POST /boards not yet wired")
}

/// POST /boards/:id/quarantine — placeholder.
pub async fn quarantine_board(_state: State<AppState>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "POST /boards/:id/quarantine not yet wired",
    )
}

/// POST /boards/:id/unquarantine — placeholder.
pub async fn unquarantine_board(_state: State<AppState>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "POST /boards/:id/unquarantine not yet wired",
    )
}
