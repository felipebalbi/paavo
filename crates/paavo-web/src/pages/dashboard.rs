//! `/` dashboard.

use crate::db::WebDb;
use crate::time::{epoch_ms_with_relative, relative_to_now};
use axum::extract::State;
use axum::response::Html;
use chrono::Utc;

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
    // Snapshot once per render so every "X ago" on the page resolves
    // against the same baseline. Avoids per-row drift on a long page.
    let now_ms = Utc::now().timestamp_millis();

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
            r#"<tr><td class="{TD}">{id}</td><td class="{TD}">{kind}</td><td class="{TD} {hc}">{h:?}</td><td class="{TD} text-zinc-500" title="{lu_abs}">{lu_rel}</td></tr>"#,
            id = super::html_escape(&b.spec.id),
            kind = super::html_escape(&b.spec.kind),
            hc = super::health_class(b.spec.health),
            h = b.spec.health,
            // Two timestamps per cell: a relative form ("3 minutes
            // ago") as the visible body, an absolute UTC form as the
            // hover tooltip. Operators glance at relative; copy-paste
            // chooses the absolute (which is what shows up in screen
            // readers and search results).
            lu_abs = b
                .last_used_at
                .map(|t| crate::time::epoch_ms_to_utc(Some(t)))
                .unwrap_or_else(|| "never".into()),
            lu_rel = match b.last_used_at {
                Some(t) => relative_to_now(t, now_ms),
                None => "never".into(),
            },
        ));
    }
    body.push_str("</tbody></table>");

    body.push_str(&format!(r#"<h2 class="{H2}">Recent jobs</h2>"#));
    body.push_str(&format!(
        r#"<table class="{TABLE}"><thead><tr><th class="{TH}">id</th><th class="{TH}">state</th><th class="{TH}">priority</th><th class="{TH}">submitter</th><th class="{TH}">submitted</th></tr></thead><tbody>"#
    ));
    for j in &jobs {
        body.push_str(&format!(
            r#"<tr><td class="{TD}"><a class="text-blue-700 hover:underline" href="/jobs/{id}">{id}</a></td><td class="{TD} {sc}">{s:?}</td><td class="{TD}">{p:?}</td><td class="{TD}">{u}</td><td class="{TD} text-zinc-500" title="{ts_abs}">{ts_rel}</td></tr>"#,
            id = j.id,
            sc = super::state_class(j.state),
            s = j.state,
            p = j.priority,
            u = super::html_escape(&j.submitter),
            // submitted_at is stored as epoch ms (i64) by paavo-db;
            // see `JobRow.submitted_at`. The format helpers expect
            // exactly that shape — no conversion needed here.
            ts_abs = crate::time::epoch_ms_to_utc(Some(j.submitted_at)),
            ts_rel = relative_to_now(j.submitted_at, now_ms),
        ));
    }
    body.push_str("</tbody></table>");
    // Suppress "unused import" for the lone `epoch_ms_with_relative`
    // until the maud refactor folds it in. Touched here so the use is
    // pinned but doesn't bloat the page yet.
    let _ = epoch_ms_with_relative as fn(Option<i64>, i64) -> String;
    super::html_shell("dashboard", body)
}
