//! `/` dashboard.

use crate::db::WebDb;
use crate::pages::NavTab;
use crate::time::relative_to_now;
use axum::extract::State;
use axum::response::Html;
use chrono::Utc;

/// Render the dashboard page.
pub async fn render(State(db): State<WebDb>) -> Html<String> {
    let boards = db.all_boards().unwrap_or_default();
    let jobs = db.recent_jobs(20).unwrap_or_default();
    // Snapshot once per render so every "X ago" on the page resolves
    // against the same baseline. Avoids per-row drift on a long page.
    let now_ms = Utc::now().timestamp_millis();

    let mut body = String::from(r#"<h1>paavo</h1>"#);
    body.push_str(&format!(
        r#"<p class="muted"><strong>{}</strong> boards · <strong>{}</strong> recent jobs</p>"#,
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
        r#"<table class="rows"><thead><tr><th>id</th><th>state</th><th>priority</th><th>submitter</th><th>submitted</th></tr></thead><tbody>"#,
    );
    if jobs.is_empty() {
        body.push_str(r#"<tr><td class="empty" colspan="5">no jobs yet</td></tr>"#);
    } else {
        for j in &jobs {
            // submitted_at is epoch ms (i64); see paavo-db's JobRow.
            let ts_abs = crate::time::epoch_ms_to_utc(Some(j.submitted_at));
            let ts_rel = relative_to_now(j.submitted_at, now_ms);
            body.push_str(&format!(
                r#"<tr><td><a href="/jobs/{id}">{id}</a></td><td class="{sc}">{s:?}</td><td>{p:?}</td><td>{u}</td><td class="ts" title="{ts_abs}">{ts_rel}</td></tr>"#,
                id = j.id,
                sc = super::state_class(j.state),
                s = j.state,
                p = j.priority,
                u = super::html_escape(&j.submitter),
            ));
        }
    }
    body.push_str("</tbody></table>");
    super::html_shell(NavTab::Dashboard, "dashboard", body)
}
