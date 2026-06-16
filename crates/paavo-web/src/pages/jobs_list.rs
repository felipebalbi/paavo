//! `/jobs`.

use crate::db::WebDb;
use crate::pages::NavTab;
use crate::time::{epoch_ms_to_utc, relative_to_now};
use axum::extract::{Query, State};
use axum::response::Html;
use chrono::Utc;
use std::collections::HashMap;

/// Render the jobs index page.
pub async fn render(
    State(db): State<WebDb>,
    Query(q): Query<HashMap<String, String>>,
) -> Html<String> {
    let limit: u32 = q
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(100)
        .min(500);
    let jobs = db.recent_jobs(limit).unwrap_or_default();
    // Per-render baseline; matches dashboard.rs.
    let now_ms = Utc::now().timestamp_millis();
    let mut body = format!(
        r#"<h1>jobs <span class="muted">(last {})</span></h1>"#,
        jobs.len()
    );
    body.push_str(
        r#"<table class="rows"><thead><tr>
<th>id</th>
<th>state</th>
<th>priority</th>
<th>submitter</th>
<th>submitted</th>
</tr></thead><tbody>"#,
    );
    if jobs.is_empty() {
        body.push_str(r#"<tr><td class="empty" colspan="5">no jobs yet</td></tr>"#);
    } else {
        for j in &jobs {
            // submitted_at is epoch ms (i64); see paavo-db's JobRow.
            // Visible cell = relative ("3 minutes ago"), tooltip = absolute UTC.
            let ts_abs = epoch_ms_to_utc(Some(j.submitted_at));
            let ts_rel = relative_to_now(j.submitted_at, now_ms);
            body.push_str(&format!(
                r#"<tr>
<td><a href="/jobs/{id}">{id}</a></td>
<td class="{sc}">{s:?}</td>
<td>{p:?}</td>
<td>{u}</td>
<td class="ts" title="{ts_abs}">{ts_rel}</td>
</tr>"#,
                id = j.id,
                sc = super::state_class(j.state),
                s = j.state,
                p = j.priority,
                u = super::html_escape(&j.submitter),
            ));
        }
    }
    body.push_str("</tbody></table>");
    super::html_shell(NavTab::Jobs, "jobs", body)
}
