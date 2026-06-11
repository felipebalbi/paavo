//! `/schedule`.

use crate::db::WebDb;
use axum::extract::State;
use axum::response::Html;

/// Render the schedule page.
pub async fn render(State(db): State<WebDb>) -> Html<String> {
    let rows = db.all_schedules().unwrap_or_default();
    let mut body = String::from(
        r#"<h1 class="text-2xl font-semibold mb-4">schedule</h1>
<table class="w-full text-sm"><thead><tr>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">id</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">cron</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">enabled</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">last triggered</th>
<th class="text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300">last completed</th>
</tr></thead><tbody>"#,
    );
    if rows.is_empty() {
        body.push_str(
            r#"<tr><td colspan="5" class="py-3 text-zinc-500 italic">no schedules registered yet — paavod's nightly cron writes a row on first fire</td></tr>"#,
        );
    } else {
        for s in &rows {
            body.push_str(&format!(
                r#"<tr>
<td class="py-1.5 border-b border-zinc-200">{id}</td>
<td class="py-1.5 border-b border-zinc-200"><code class="bg-zinc-100 px-1 rounded">{cron}</code></td>
<td class="py-1.5 border-b border-zinc-200 {ec}">{en}</td>
<td class="py-1.5 border-b border-zinc-200 text-zinc-500">{lt}</td>
<td class="py-1.5 border-b border-zinc-200 text-zinc-500">{lc}</td>
</tr>"#,
                id = super::html_escape(&s.id),
                cron = super::html_escape(&s.cron),
                ec = if s.enabled {
                    "text-emerald-700"
                } else {
                    "text-zinc-500"
                },
                en = if s.enabled { "enabled" } else { "disabled" },
                lt = s
                    .last_triggered_at
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "—".into()),
                lc = s
                    .last_completed_at
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "—".into()),
            ));
        }
    }
    body.push_str("</tbody></table>");
    super::html_shell("schedule", body)
}
