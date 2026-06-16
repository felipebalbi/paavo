//! `/schedule`.

use crate::db::WebDb;
use crate::pages::NavTab;
use crate::time::{epoch_ms_to_utc, relative_to_now};
use axum::extract::State;
use axum::response::Html;
use chrono::Utc;

/// Render the schedule page.
pub async fn render(State(db): State<WebDb>) -> Html<String> {
    let rows = db.all_schedules().unwrap_or_default();
    // Per-render baseline; matches dashboard.rs / boards.rs.
    let now_ms = Utc::now().timestamp_millis();
    let mut body = String::from(
        r#"<h1>schedule</h1>
<table class="rows"><thead><tr>
<th>id</th>
<th>cron</th>
<th>enabled</th>
<th>last triggered</th>
<th>last completed</th>
</tr></thead><tbody>"#,
    );
    if rows.is_empty() {
        body.push_str(
            r#"<tr><td class="empty" colspan="5">no schedules registered yet — paavod's nightly cron writes a row on first fire</td></tr>"#,
        );
    } else {
        for s in &rows {
            // Two-faced timestamps for last_triggered_at and
            // last_completed_at: visible body is relative ("2 hours
            // ago"), tooltip is absolute UTC. Same pattern as boards.rs.
            let (lt_abs, lt_rel) = match s.last_triggered_at {
                Some(t) => (epoch_ms_to_utc(Some(t)), relative_to_now(t, now_ms)),
                None => ("never".into(), "never".into()),
            };
            let (lc_abs, lc_rel) = match s.last_completed_at {
                Some(t) => (epoch_ms_to_utc(Some(t)), relative_to_now(t, now_ms)),
                None => ("never".into(), "never".into()),
            };
            body.push_str(&format!(
                r#"<tr>
<td>{id}</td>
<td><code>{cron}</code></td>
<td class="{ec}">{en}</td>
<td class="ts" title="{lt_abs}">{lt_rel}</td>
<td class="ts" title="{lc_abs}">{lc_rel}</td>
</tr>"#,
                id = super::html_escape(&s.id),
                cron = super::html_escape(&s.cron),
                ec = if s.enabled { "enabled" } else { "disabled" },
                en = if s.enabled { "enabled" } else { "disabled" },
            ));
        }
    }
    body.push_str("</tbody></table>");
    super::html_shell(NavTab::Schedule, "schedule", body)
}
