//! `/api/jobs*` JSON handlers.
//!
//! - `GET /api/jobs` reads SQLite directly: fuzzy search via the
//!   `fuzzy_score` function, or a time-ordered page for a blank query.
//!   The live `jobs` revision still comes from the poller.
//! - `GET /api/jobs/:id` and `GET /api/jobs/:id/log` read the RO sqlite
//!   handle directly (single-row / bounded-page queries; see the
//!   rationale on [`crate::db`]).
use crate::db::WebDb;
use crate::proxy::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use paavo_proto::{JobId, JobListItem, JobView, LogFrame, Page};
use std::collections::HashMap;
use std::str::FromStr;

/// `GET /api/jobs?q=&page=&per_page=&as_of=` — paginated jobs read
/// straight from the RO sqlite handle.
///
/// Blank `q` returns the time-ordered list (optionally pinned to
/// `submitted_at <= as_of` so a paging session sees a stable window),
/// with `new_count` reporting rows newer than `as_of`. A non-blank `q`
/// switches to fuzzy ranking via the `fuzzy_score` SQL function;
/// `new_count` is meaningless there and forced to 0. The `jobs`
/// revision still comes from the live poller so the SPA can de-dup
/// against the `jobs` SSE event. Each `WebDb` call takes and drops its
/// own short lock, and this handler has no `.await` between them, so it
/// never holds a lock across a suspension point.
pub async fn list(
    State(s): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Page<JobListItem>>, (StatusCode, String)> {
    let page: u32 = q
        .get("page")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1);
    let per_page: u32 = q
        .get("per_page")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
        .clamp(1, 200);
    // Trim so `?q=` (or trailing spaces) behaves like no filter.
    let query = q.get("q").map(|v| v.trim().to_string()).unwrap_or_default();
    let as_of: Option<i64> = q.get("as_of").and_then(|v| v.parse().ok());
    let err = |e: paavo_db::DbError| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    // saturating_mul: an unclamped hostile `?page=` must yield an empty
    // page, not overflow.
    let offset = (page - 1).saturating_mul(per_page);

    let (items, total, new_count) = if query.is_empty() {
        let items = s.db.jobs_list_page(as_of, offset, per_page).map_err(err)?;
        let total = s.db.jobs_count(as_of).map_err(err)?;
        let new_count = s.db.jobs_new_count(as_of).map_err(err)?;
        (items, total, new_count)
    } else {
        let items =
            s.db.jobs_search_page(&query, offset, per_page)
                .map_err(err)?;
        let total = s.db.jobs_search_count(&query).map_err(err)?;
        (items, total, 0)
    };

    Ok(Json(Page {
        items,
        total,
        page,
        per_page,
        revision: s.live.revisions().jobs,
        new_count,
        as_of,
    }))
}

/// `GET /api/jobs/:id` — one job (404 if unknown, 400 if not a ULID).
pub async fn get(
    State(db): State<WebDb>,
    Path(id): Path<String>,
) -> Result<Json<JobView>, (StatusCode, String)> {
    let id =
        JobId::from_str(&id).map_err(|_| (StatusCode::BAD_REQUEST, "invalid job id".into()))?;
    match db
        .job(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        Some(r) => Ok(Json(row_to_view(r))),
        None => Err((StatusCode::NOT_FOUND, "no such job".into())),
    }
}

/// `GET /api/jobs/:id/log?offset=&limit=` — historical persisted frames
/// (oldest first). The live tail is a separate concern served by
/// [`crate::proxy::stream_job`]; this endpoint backfills the scrollback
/// the SPA renders before (or instead of) attaching the live stream.
pub async fn log(
    State(db): State<WebDb>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Vec<LogFrame>>, (StatusCode, String)> {
    let id =
        JobId::from_str(&id).map_err(|_| (StatusCode::BAD_REQUEST, "invalid job id".into()))?;
    let offset: u32 = q.get("offset").and_then(|v| v.parse().ok()).unwrap_or(0);
    let limit: u32 = q
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000)
        .clamp(1, 5000);
    let frames = db
        .job_logs_page(&id, offset, limit)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(frames))
}

/// Project a `paavo_db::JobRow` onto the wire [`JobView`], dropping the
/// server-local filesystem paths (`tar_path`, `elf_path`) that the
/// daemon must not leak to HTTP clients.
fn row_to_view(r: paavo_db::JobRow) -> paavo_proto::JobView {
    paavo_proto::JobView {
        id: r.id,
        priority: r.priority,
        submitter: r.submitter,
        source: r.source,
        board_selector: r.board_selector,
        inactivity_timeout_ms: r.inactivity_timeout_ms,
        hard_max_ms: r.hard_max_ms,
        state: r.state,
        outcome: r.outcome,
        board_id: r.board_id,
        submitted_at: r.submitted_at,
        started_at: r.started_at,
        finished_at: r.finished_at,
        tar_blake3: r.tar_blake3,
        cargo_update_packages: r.cargo_update_packages,
        skip_cache: r.skip_cache,
    }
}
