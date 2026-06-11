//! `/` dashboard.

use crate::db::WebDb;
use axum::extract::State;
use axum::response::Html;

/// Shared utility-class snippets used across this file.
const H1: &str = "text-2xl font-semibold mb-4";
const H2: &str = "text-lg font-semibold mt-8 mb-2 text-zinc-700";
const TABLE: &str = "w-full text-sm";
const TH: &str = "text-left font-semibold text-zinc-600 py-1.5 border-b border-zinc-300";
const TD: &str = "py-1.5 border-b border-zinc-200";

/// Render the dashboard page.
pub async fn render(State(db): State<WebDb>) -> Html<String> {
    let boards = db.all_boards().unwrap_or_default();
    let jobs = db.recent_jobs(20).unwrap_or_default();

    let mut body = format!(r#"<h1 class="{H1}">paavo</h1>"#);
    body.push_str(&format!(
        r#"<p class="text-zinc-600"><span class="font-semibold text-zinc-900">{}</span> boards · <span class="font-semibold text-zinc-900">{}</span> recent jobs</p>"#,
        boards.len(),
        jobs.len()
    ));
    body.push_str(&format!(r#"<h2 class="{H2}">Board fleet</h2>"#));
    body.push_str(&format!(
        r#"<table class="{TABLE}"><thead><tr><th class="{TH}">id</th><th class="{TH}">kind</th><th class="{TH}">health</th><th class="{TH}">last used</th></tr></thead><tbody>"#
    ));
    for b in &boards {
        body.push_str(&format!(
            r#"<tr><td class="{TD}">{id}</td><td class="{TD}">{kind}</td><td class="{TD} {hc}">{h:?}</td><td class="{TD}">{lu}</td></tr>"#,
            id = super::html_escape(&b.spec.id),
            kind = super::html_escape(&b.spec.kind),
            hc = super::health_class(b.spec.health),
            h = b.spec.health,
            lu = b
                .last_used_at
                .map(|t| t.to_string())
                .unwrap_or_else(|| "—".into()),
        ));
    }
    body.push_str("</tbody></table>");

    body.push_str(&format!(r#"<h2 class="{H2}">Recent jobs</h2>"#));
    body.push_str(&format!(
        r#"<table class="{TABLE}"><thead><tr><th class="{TH}">id</th><th class="{TH}">state</th><th class="{TH}">priority</th><th class="{TH}">submitter</th></tr></thead><tbody>"#
    ));
    for j in &jobs {
        body.push_str(&format!(
            r#"<tr><td class="{TD}"><a class="text-blue-700 hover:underline" href="/jobs/{id}">{id}</a></td><td class="{TD} {sc}">{s:?}</td><td class="{TD}">{p:?}</td><td class="{TD}">{u}</td></tr>"#,
            id = j.id,
            sc = super::state_class(j.state),
            s = j.state,
            p = j.priority,
            u = super::html_escape(&j.submitter),
        ));
    }
    body.push_str("</tbody></table>");
    super::html_shell("dashboard", body)
}
