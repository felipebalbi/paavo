//! `/jobs`.

use crate::db::WebDb;
use axum::extract::{Query, State};
use axum::response::Html;
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
    let mut body = format!(
        r#"<h1 class="text-2xl font-semibold mb-4">jobs <span class="text-zinc-500 font-normal text-base">(last {})</span></h1>"#,
        jobs.len()
    );
    body.push_str(
        r#"<table class="w-full text-sm"><thead><tr>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">id</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">state</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">priority</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">submitter</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">submitted</th>
</tr></thead><tbody>"#,
    );
    for j in &jobs {
        body.push_str(&format!(
            r#"<tr>
<td class="py-1.5 border-b border-zinc-200"><a class="text-blue-700 hover:underline" href="/jobs/{id}">{id}</a></td>
<td class="py-1.5 border-b border-zinc-200 {sc}">{s:?}</td>
<td class="py-1.5 border-b border-zinc-200">{p:?}</td>
<td class="py-1.5 border-b border-zinc-200">{u}</td>
<td class="py-1.5 border-b border-zinc-200 text-zinc-500">{ts}</td>
</tr>"#,
            id = j.id,
            sc = super::state_class(j.state),
            s = j.state,
            p = j.priority,
            u = super::html_escape(&j.submitter),
            ts = j.submitted_at,
        ));
    }
    body.push_str("</tbody></table>");
    super::html_shell("jobs", body)
}
