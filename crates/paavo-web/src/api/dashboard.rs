//! `GET /api/dashboard` — the consolidated landing-page payload.
//!
//! One bounded response for the dashboard: exact SQL aggregate counts
//! (`job_state_counts`, `board_health_counts`) plus the two short display
//! lists the page renders — the 8 newest jobs (from the poller-maintained
//! in-memory index, so the jobs list never touches sqlite on the request
//! path) and the relevant fleet slice (`boards_dashboard`, SQL). Its size
//! does not grow with the fleet or job history.
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
/// `AppState`: it needs the DB (counts + fleet slice) and the live state
/// (recent-jobs index + current revisions). There is no `.await` between
/// taking the index read-guard and dropping it, so no lock is held across
/// a suspension point.
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
    let recent_jobs = {
        let (items, _) = s.live.index.read().search("", None, 1, RECENT_JOBS);
        items
    };
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
