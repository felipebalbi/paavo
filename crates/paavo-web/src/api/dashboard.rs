//! `GET /api/dashboard` — the consolidated landing-page payload.
//!
//! One bounded response for the dashboard: exact SQL aggregate counts
//! (`job_state_counts`, `board_health_counts`) plus the two short display
//! lists the page renders — the 8 newest jobs (`jobs_list_page`, SQL) and
//! the relevant fleet slice (`boards_dashboard`, SQL). Its size does not
//! grow with the fleet or job history.
use crate::api::boards::board_view;
use crate::proxy::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use paavo_proto::DashboardOverview;

/// Newest jobs shown in the "Recent activity" table.
const RECENT_JOBS: u32 = 8;
/// Boards shown in the "Board fleet" table (quarantined-first, LRU).
const FLEET_SLICE: u32 = 8;

/// `GET /api/dashboard` — see the module docs. Extracts the whole
/// `AppState`: it needs the DB (counts, recent jobs, and fleet slice) and
/// the live state (current revisions). Each `s.db` call locks the RO
/// connection only for its own query, so no lock is held across an
/// `.await`.
pub async fn get(
    State(s): State<AppState>,
) -> Result<Json<DashboardOverview>, (StatusCode, String)> {
    let err = |e: paavo_db::DbError| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let jobs = s.db.job_state_counts().map_err(err)?;
    let boards = s.db.board_health_counts().map_err(err)?;
    let fleet =
        s.db.boards_dashboard(FLEET_SLICE)
            .map_err(err)?
            .into_iter()
            .map(board_view)
            .collect();
    // Newest-first, capped — the SQL counterpart to the old in-memory index
    // read (no `as_of` pin: the dashboard always wants the latest jobs).
    let recent_jobs = s.db.jobs_list_page(None, 0, RECENT_JOBS).map_err(err)?;
    let rev = s.live.revisions();
    Ok(Json(DashboardOverview {
        jobs,
        boards,
        recent_jobs,
        fleet,
        jobs_revision: rev.jobs,
        boards_revision: rev.boards,
    }))
}
