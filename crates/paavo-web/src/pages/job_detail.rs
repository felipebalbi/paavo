//! `/jobs/:id`.

use crate::db::WebDb;
use axum::extract::{Path, State};
use axum::response::Html;
use std::str::FromStr;

/// How many log frames to fetch from sqlite and render on the detail
/// page. Used for both the DB query and the render-side cap so the two
/// can never drift (fetching 5000 and only rendering 2000 wastes both
/// I/O and memory).
const LOG_PAGE_LIMIT: u32 = 2000;

/// Render the job detail page.
pub async fn render(State(db): State<WebDb>, Path(id): Path<String>) -> Html<String> {
    let id = match paavo_proto::JobId::from_str(&id) {
        Ok(i) => i,
        Err(_) => {
            return super::html_shell("job", r#"<p class="text-rose-700">invalid id</p>"#.into())
        }
    };
    let job = match db.job(&id).ok().flatten() {
        Some(j) => j,
        None => {
            return super::html_shell("job", r#"<p class="text-rose-700">not found</p>"#.into())
        }
    };
    let logs = db.job_logs(&id, LOG_PAGE_LIMIT).unwrap_or_default();
    let mut body = format!(
        r#"<h1 class="text-2xl font-semibold mb-2">job <code class="text-base bg-zinc-100 px-2 py-0.5 rounded">{id}</code></h1>
<p class="text-zinc-600 mb-4">state: <span class="font-semibold {sc}">{state:?}</span> · priority: <span class="text-zinc-900 font-semibold">{prio:?}</span> · submitter: <span class="text-zinc-900">{sub}</span></p>"#,
        id = job.id,
        sc = super::state_class(job.state),
        state = job.state,
        prio = job.priority,
        sub = super::html_escape(&job.submitter),
    );
    if let Some(o) = &job.outcome {
        body.push_str(&format!(
            r#"<p class="mb-4">outcome: <code class="bg-zinc-100 px-2 py-0.5 rounded text-sm">{}</code></p>"#,
            super::html_escape(&serde_json::to_string(o).unwrap_or_default())
        ));
    }
    body.push_str(r#"<h2 class="text-lg font-semibold mt-6 mb-2 text-zinc-700">log</h2>"#);
    body.push_str(
        r#"<pre class="bg-zinc-900 text-zinc-100 text-xs leading-snug p-4 rounded overflow-x-auto">"#,
    );
    for f in &logs {
        body.push_str(&format!(
            "{:>10} [{:?}] {}\n",
            f.ts_us,
            f.level,
            super::html_escape(&f.message)
        ));
    }
    body.push_str("</pre>");
    super::html_shell("job", body)
}
