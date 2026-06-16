//! `/` dashboard.

use crate::db::WebDb;
use crate::pages::NavTab;
use crate::time::relative_to_now;
use axum::extract::State;
use axum::response::Html;
use chrono::Utc;

/// Cap on rows shown in the "Recent jobs" table. Shared by the SSR
/// dashboard render and the live feed so their row sets + counts match.
pub(crate) const RECENT_JOBS_LIMIT: u32 = 20;

/// Cargo package version, for the cache-bust on `/static/dashboard-live.js`.
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Render the inner HTML of the "Recent jobs" `<tbody>` — the `<tr>`
/// rows, or the single `no jobs yet` empty-state row. Single source of
/// truth for row markup, escaping, state classes, and relative
/// timestamps; called by both `render` (SSR) and the live feed.
pub(crate) fn recent_jobs_tbody(jobs: &[paavo_db::JobRow], now_ms: i64) -> String {
    if jobs.is_empty() {
        return r#"<tr><td class="empty" colspan="5">no jobs yet</td></tr>"#.to_string();
    }
    let mut out = String::new();
    for j in jobs {
        // submitted_at is epoch ms (i64); see paavo-db's JobRow.
        let ts_abs = crate::time::epoch_ms_to_utc(Some(j.submitted_at));
        let ts_rel = relative_to_now(j.submitted_at, now_ms);
        out.push_str(&format!(
            r#"<tr><td><a href="/jobs/{id}">{id}</a></td><td class="{sc}">{s:?}</td><td>{p:?}</td><td>{u}</td><td class="ts" title="{ts_abs}">{ts_rel}</td></tr>"#,
            id = j.id,
            sc = super::state_class(j.state),
            s = j.state,
            p = j.priority,
            u = super::html_escape(&j.submitter),
        ));
    }
    out
}

/// Render the dashboard page.
pub async fn render(State(db): State<WebDb>) -> Html<String> {
    let boards = db.all_boards().unwrap_or_default();
    let jobs = db.recent_jobs(RECENT_JOBS_LIMIT).unwrap_or_default();
    // Snapshot once per render so every "X ago" on the page resolves
    // against the same baseline. Avoids per-row drift on a long page.
    let now_ms = Utc::now().timestamp_millis();

    let mut body = String::from(r#"<h1>paavo</h1>"#);
    body.push_str(&format!(
        r#"<p class="muted"><strong>{}</strong> boards · <strong id="recent-jobs-count">{}</strong> recent jobs</p>"#,
        boards.len(),
        jobs.len()
    ));

    // Board fleet
    body.push_str(r#"<h2>Board fleet</h2>"#);
    body.push_str(
        r#"<table class="rows"><thead><tr><th>id</th><th>kind</th><th>health</th><th>last used</th></tr></thead><tbody>"#,
    );
    if boards.is_empty() {
        body.push_str(r#"<tr><td class="empty" colspan="4">no boards registered</td></tr>"#);
    } else {
        for b in &boards {
            // Two timestamps per cell: relative form ("3 minutes ago")
            // as the visible body, absolute UTC as the hover tooltip
            // for screenshots and copy-paste. See pages/dashboard.rs
            // commit a (commit 5cc0ab7) for the rationale.
            let (lu_abs, lu_rel) = match b.last_used_at {
                Some(t) => (
                    crate::time::epoch_ms_to_utc(Some(t)),
                    relative_to_now(t, now_ms),
                ),
                None => ("never".into(), "never".into()),
            };
            body.push_str(&format!(
                r#"<tr><td>{id}</td><td>{kind}</td><td class="{hc}">{h:?}</td><td class="ts" title="{lu_abs}">{lu_rel}</td></tr>"#,
                id = super::html_escape(&b.spec.id),
                kind = super::html_escape(&b.spec.kind),
                hc = super::health_class(b.spec.health),
                h = b.spec.health,
            ));
        }
    }
    body.push_str("</tbody></table>");

    // Recent jobs
    body.push_str(r#"<h2>Recent jobs</h2>"#);
    body.push_str(
        r#"<table class="rows"><thead><tr><th>id</th><th>state</th><th>priority</th><th>submitter</th><th>submitted</th></tr></thead><tbody id="recent-jobs-body">"#,
    );
    body.push_str(&recent_jobs_tbody(&jobs, now_ms));
    body.push_str("</tbody></table>");
    body.push_str(&format!(
        r#"<script src="/static/dashboard-live.js?v={PKG_VERSION}"></script>"#
    ));
    super::html_shell(NavTab::Dashboard, "dashboard", body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paavo_db::{Db, NewJob};
    use paavo_proto::{BoardSelector, JobId, JobSource, Priority};
    use tempfile::tempdir;

    fn sample_new_job(id: JobId) -> NewJob {
        NewJob {
            id,
            priority: Priority::Interactive,
            submitter: "alice".into(),
            source: JobSource::Cli,
            board_selector: BoardSelector {
                kind: "mcxa266".into(),
                instance: None,
                wiring_profile: None,
            },
            inactivity_timeout_ms: 120_000,
            hard_max_ms: 900_000,
            tar_blake3: "deadbeef".into(),
            tar_path: "/tmp/x.tar".into(),
            cargo_update_packages: vec![],
            skip_cache: false,
        }
    }

    #[test]
    fn recent_jobs_tbody_empty_renders_placeholder() {
        let html = recent_jobs_tbody(&[], 0);
        assert!(html.contains("no jobs yet"), "got: {html}");
    }

    #[test]
    fn recent_jobs_tbody_renders_a_job_row() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("paavo.sqlite");
        let id = JobId::new();
        {
            let db = Db::open(&path).unwrap();
            paavo_db::JobRow::insert(db.raw_conn(), &sample_new_job(id), 0).unwrap();
        }
        let webdb = crate::db::WebDb::open(&path).unwrap();
        let jobs = webdb.recent_jobs(RECENT_JOBS_LIMIT).unwrap();
        let html = recent_jobs_tbody(&jobs, 0);
        assert!(
            html.contains(&id.to_string()),
            "row missing job id; got: {html}"
        );
        assert!(
            html.contains("s-submitted"),
            "row missing state class; got: {html}"
        );
        assert!(html.contains("alice"), "row missing submitter; got: {html}");
    }
}
