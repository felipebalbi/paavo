//! `/api/schedules` JSON handler.
//!
//! Mirrors [`crate::api::boards`]: a small, slowly-changing set served
//! as a direct paginated sqlite read (id ASC), with the `revision`
//! echoed from the live poller for SSE de-dup.
use crate::proxy::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use paavo_proto::{Page, ScheduleView};
use std::collections::HashMap;

/// `GET /api/schedules?page=&per_page=` — paginated schedules (id ASC).
///
/// Takes the full [`AppState`] for the same reason as
/// [`crate::api::boards::list`]: it needs the DB rows plus the current
/// `schedules` revision. No `as_of`/`new_count` apply.
pub async fn list(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Page<ScheduleView>>, (StatusCode, String)> {
    let page: u32 = q
        .get("page")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1);
    let per_page: u32 = q
        .get("per_page")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
        .clamp(1, 100);
    let err = |e: paavo_db::DbError| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let total = s.db.schedules_count().map_err(err)?;
    // saturating_mul: guards an unclamped `page` from overflowing u32
    // (see api/boards.rs for the rationale).
    let rows =
        s.db.schedules_page((page - 1).saturating_mul(per_page), per_page)
            .map_err(err)?;
    Ok(Json(Page {
        items: rows.into_iter().map(schedule_view).collect(),
        total,
        page,
        per_page,
        revision: s.live.revisions().schedules,
        new_count: 0,
        as_of: None,
    }))
}

/// Project a `paavo_db::ScheduleRow` onto the wire [`ScheduleView`].
/// There are no server-local fields to drop.
fn schedule_view(r: paavo_db::ScheduleRow) -> paavo_proto::ScheduleView {
    paavo_proto::ScheduleView {
        id: r.id,
        cron: r.cron,
        enabled: r.enabled,
        last_triggered_at: r.last_triggered_at,
        last_completed_at: r.last_completed_at,
    }
}
