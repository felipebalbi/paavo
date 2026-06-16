//! `/boards`.

use crate::db::WebDb;
use crate::pages::NavTab;
use crate::time::{epoch_ms_to_utc, relative_to_now};
use axum::extract::State;
use axum::response::Html;
use chrono::Utc;

/// Render the boards page.
pub async fn render(State(db): State<WebDb>) -> Html<String> {
    let rows = db.all_boards().unwrap_or_default();
    // Snapshot once per render; see dashboard.rs for the rationale.
    let now_ms = Utc::now().timestamp_millis();
    let mut body = String::from(
        r#"<h1>boards</h1>
<table class="rows"><thead><tr>
<th>id</th>
<th>kind</th>
<th>health</th>
<th>infra fails</th>
<th>last used</th>
<th>reason</th>
</tr></thead><tbody>"#,
    );
    if rows.is_empty() {
        body.push_str(r#"<tr><td class="empty" colspan="6">no boards registered</td></tr>"#);
    } else {
        for b in &rows {
            // Two-faced timestamp: visible cell text is the relative
            // form (the operator's "is this stale?" glance), absolute
            // UTC is hover-only via `title`. Same pattern as dashboard.rs.
            let (lu_abs, lu_rel) = match b.last_used_at {
                Some(t) => (epoch_ms_to_utc(Some(t)), relative_to_now(t, now_ms)),
                None => ("never".into(), "never".into()),
            };
            body.push_str(&format!(
                r#"<tr>
<td>{id}</td>
<td>{k}</td>
<td class="{hc}">{h:?}</td>
<td>{n}</td>
<td class="ts" title="{lu_abs}">{lu_rel}</td>
<td class="dim">{r}</td>
</tr>"#,
                id = super::html_escape(&b.spec.id),
                k = super::html_escape(&b.spec.kind),
                hc = super::health_class(b.spec.health),
                h = b.spec.health,
                n = b.consecutive_infra_failures,
                r = super::html_escape(&b.quarantine_reason.clone().unwrap_or_default()),
            ));
        }
    }
    body.push_str("</tbody></table>");
    super::html_shell(NavTab::Boards, "boards", body)
}
