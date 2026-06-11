//! GET /health and GET /ready.
//!
//! Per spec §9.5: `/health` is liveness — always 200 with a small JSON
//! body, even while draining. `/ready` is readiness — 503 while draining,
//! 200 otherwise. Liveness probes (systemd Watchdog, k8s) must not kill
//! the daemon mid-drain.

use crate::app_state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;

/// Body for both endpoints.
#[derive(Serialize)]
pub struct HealthBody {
    /// Always `"paavod"`.
    pub service: &'static str,
    /// True while not draining. Used by both `/health` and `/ready`,
    /// but only `/ready` flips its HTTP status based on it.
    pub ready: bool,
    /// Crate version.
    pub version: &'static str,
}

fn body(ready: bool) -> HealthBody {
    HealthBody {
        service: "paavod",
        ready,
        version: env!("CARGO_PKG_VERSION"),
    }
}

/// Liveness — always 200, even while draining. Body reports the drain
/// state so monitoring can observe it without flipping the probe.
pub async fn health(State(s): State<AppState>) -> impl IntoResponse {
    (StatusCode::OK, Json(body(!s.drain.is_draining())))
}

/// Readiness — 503 while draining, 200 otherwise.
pub async fn ready(State(s): State<AppState>) -> impl IntoResponse {
    let draining = s.drain.is_draining();
    let status = if draining {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    (status, Json(body(!draining)))
}
