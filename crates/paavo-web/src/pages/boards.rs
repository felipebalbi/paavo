//! `/boards`.

use crate::db::WebDb;
use axum::extract::State;
use axum::response::Html;

/// Render the boards page.
pub async fn render(State(db): State<WebDb>) -> Html<String> {
    let rows = db.all_boards().unwrap_or_default();
    let mut body = String::from(
        r#"<h1 class="text-2xl font-semibold mb-4">boards</h1>
<table class="w-full text-sm"><thead><tr>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">id</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">kind</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">health</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">infra fails</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">last used</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">reason</th>
</tr></thead><tbody>"#,
    );
    for b in &rows {
        body.push_str(&format!(
            r#"<tr>
<td class="py-1.5 border-b border-zinc-200">{id}</td>
<td class="py-1.5 border-b border-zinc-200">{k}</td>
<td class="py-1.5 border-b border-zinc-200 {hc}">{h:?}</td>
<td class="py-1.5 border-b border-zinc-200">{n}</td>
<td class="py-1.5 border-b border-zinc-200 text-zinc-500">{lu}</td>
<td class="py-1.5 border-b border-zinc-200 text-zinc-500">{r}</td>
</tr>"#,
            id = super::html_escape(&b.spec.id),
            k = super::html_escape(&b.spec.kind),
            hc = super::health_class(b.spec.health),
            h = b.spec.health,
            n = b.consecutive_infra_failures,
            lu = b
                .last_used_at
                .map(|t| t.to_string())
                .unwrap_or_else(|| "—".into()),
            r = super::html_escape(&b.quarantine_reason.clone().unwrap_or_default()),
        ));
    }
    body.push_str("</tbody></table>");
    super::html_shell("boards", body)
}
