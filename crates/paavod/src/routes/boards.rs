//! /boards/* handlers.
//!
//! Lock ordering: every mutating handler takes `s.db.lock()` for the
//! mutation, drops the guard, then calls `refresh_inventory(&s)` which
//! locks `s.db` and `s.inventory` together (db → inventory) to
//! atomically read the table and replace the cache. Holding both
//! together inside `refresh_inventory` means two concurrent writers
//! cannot interleave their reads/writes and leave a stale snapshot.
//!
//! If `refresh_inventory` fails after a successful mutation, the cache
//! is briefly stale but the DB is the source of truth and the next
//! successful write (or paavod's startup hydration) will reconverge.
//! We therefore log + warn on refresh failure but still report the
//! mutation as successful — never return 500 to a caller whose write
//! actually committed.

use crate::app_state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use paavo_db::DbError;
use paavo_proto::{BoardHealth, BoardSpec, BoardView};
use serde::Deserialize;
use tracing::{error, warn};

/// Shorthand for handler results. Errors carry an HTTP status + a
/// stable text/plain message. A richer JSON envelope is a future
/// upgrade (tracked separately).
type HandlerResult<T> = Result<T, (StatusCode, String)>;

/// GET /boards. Returns `Vec<BoardView>` so callers see the same
/// operational fields the spec promises (last-used, quarantine reason,
/// infra-failure counter, created-at).
pub async fn list_boards(State(s): State<AppState>) -> HandlerResult<Json<Vec<BoardView>>> {
    let rows = paavo_db::BoardRow::list_all(s.db.lock().raw_conn()).map_err(db_to_http)?;
    let views: Vec<BoardView> = rows.into_iter().map(row_to_view).collect();
    Ok(Json(views))
}

/// POST /boards. Body is a `BoardSpec`; `health` must be `Healthy` —
/// quarantine flows through the dedicated endpoint so `quarantine_reason`
/// can never be `NULL` for a quarantined row.
pub async fn add_board(
    State(s): State<AppState>,
    Json(spec): Json<BoardSpec>,
) -> HandlerResult<StatusCode> {
    if spec.health != BoardHealth::Healthy {
        return Err((
            StatusCode::BAD_REQUEST,
            "board must be registered as `healthy`; use POST \
             /boards/:id/quarantine to quarantine after creation"
                .into(),
        ));
    }
    let now_ms = Utc::now().timestamp_millis();
    {
        let db = s.db.lock();
        paavo_db::BoardRow::insert(db.raw_conn(), &spec, now_ms).map_err(db_to_http)?;
    }
    refresh_inventory_lossy(&s);
    Ok(StatusCode::CREATED)
}

/// Body for `POST /boards/:id/quarantine`.
#[derive(Deserialize)]
pub struct QuarantineBody {
    /// Human-readable reason. Whitespace-only is rejected.
    pub reason: String,
}

/// POST /boards/:id/quarantine.
pub async fn quarantine_board(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<QuarantineBody>,
) -> HandlerResult<StatusCode> {
    let reason = body.reason.trim();
    if reason.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "`reason` is required and must not be whitespace-only".into(),
        ));
    }
    {
        let db = s.db.lock();
        paavo_db::BoardRow::quarantine(db.raw_conn(), &id, reason).map_err(db_to_http)?;
    }
    refresh_inventory_lossy(&s);
    Ok(StatusCode::NO_CONTENT)
}

/// POST /boards/:id/unquarantine.
pub async fn unquarantine_board(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<StatusCode> {
    {
        let db = s.db.lock();
        paavo_db::BoardRow::unquarantine(db.raw_conn(), &id).map_err(db_to_http)?;
    }
    refresh_inventory_lossy(&s);
    Ok(StatusCode::NO_CONTENT)
}

/// Re-read the `boards` table and replace the cached inventory. Takes
/// both locks (db then inventory) so the read+write is atomic with
/// respect to other writers. Called by `paavod::main` at startup to
/// hydrate the initial snapshot — that's why this is `pub(crate)`.
pub(crate) fn refresh_inventory(s: &AppState) -> paavo_db::Result<()> {
    let db = s.db.lock();
    let rows = paavo_db::BoardRow::list_all(db.raw_conn())?;
    let mut inv = s.inventory.lock();
    *inv = rows.into_iter().map(|r| r.spec).collect();
    Ok(())
}

/// Refresh the inventory but never fail the caller's request. If the
/// refresh fails the DB still holds the truth; the cache will
/// reconverge on the next successful mutation or on paavod restart.
fn refresh_inventory_lossy(s: &AppState) {
    if let Err(e) = refresh_inventory(s) {
        warn!(error = %e, "inventory cache refresh failed; DB is still authoritative");
    }
}

fn row_to_view(r: paavo_db::BoardRow) -> BoardView {
    BoardView {
        spec: r.spec,
        quarantine_reason: r.quarantine_reason,
        consecutive_infra_failures: r.consecutive_infra_failures,
        last_used_at: r.last_used_at,
        created_at: r.created_at,
    }
}

/// Map a `DbError` to an HTTP status + message. Typed variants
/// (`NotFound`, `AlreadyExists`) get 404/409 with their own messages;
/// everything else becomes 500 with the `Display` text (info-leak
/// risk on `Sqlite(...)` is acceptable for an internal lab tool but
/// we log the full error so it's not silently lost).
fn db_to_http(err: DbError) -> (StatusCode, String) {
    match err {
        DbError::NotFound { entity, id } => {
            (StatusCode::NOT_FOUND, format!("{entity} not found: {id}"))
        }
        DbError::AlreadyExists { entity, id } => (
            StatusCode::CONFLICT,
            format!("{entity} already exists: {id}"),
        ),
        other => {
            error!(error = ?other, "unexpected db error");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{other}"))
        }
    }
}
