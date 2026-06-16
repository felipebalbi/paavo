//! `/api/boards` JSON handler.
//!
//! Boards are a small, slowly-changing set, so this is a direct
//! paginated sqlite read (id ASC). The `revision` echoed on the page
//! comes from the live poller so the SPA can de-dup against the
//! `boards` SSE event.
use crate::proxy::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use paavo_proto::{BoardView, Page};
use std::collections::HashMap;

/// `GET /api/boards?page=&per_page=` — paginated boards (id ASC).
///
/// Takes the full [`AppState`] because it needs both the DB handle
/// (the rows) and the live state (the current boards revision). There
/// is no `as_of` cursor or `new_count` for boards, so both are 0/None.
pub async fn list(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Page<BoardView>>, (StatusCode, String)> {
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
    let total = s.db.boards_count().map_err(err)?;
    // saturating_mul: `page` is unclamped above, so a hostile
    // `?page=4000000000` would overflow u32 (panic in debug, wrap in
    // release). Saturating to u32::MAX yields an empty page instead.
    let rows =
        s.db.boards_page((page - 1).saturating_mul(per_page), per_page)
            .map_err(err)?;
    Ok(Json(Page {
        items: rows.into_iter().map(board_view).collect(),
        total,
        page,
        per_page,
        revision: s.live.revisions().boards,
        new_count: 0,
        as_of: None,
    }))
}

/// Project a `paavo_db::BoardRow` onto the wire [`BoardView`]. The
/// `spec` is flattened on the wire (see the `BoardView` definition);
/// there are no server-local fields to drop.
fn board_view(r: paavo_db::BoardRow) -> paavo_proto::BoardView {
    paavo_proto::BoardView {
        spec: r.spec,
        quarantine_reason: r.quarantine_reason,
        consecutive_infra_failures: r.consecutive_infra_failures,
        last_used_at: r.last_used_at,
        created_at: r.created_at,
    }
}
