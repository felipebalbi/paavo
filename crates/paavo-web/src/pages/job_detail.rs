//! `/jobs/:id`.

use crate::db::WebDb;
use crate::pages::NavTab;
use crate::time::{relative_us, ts_us_to_wall_clock};
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
            return super::html_shell(
                NavTab::Jobs,
                "job",
                r#"<p class="s-failed">invalid id</p>"#.into(),
            )
        }
    };
    let job = match db.job(&id).ok().flatten() {
        Some(j) => j,
        None => {
            return super::html_shell(
                NavTab::Jobs,
                "job",
                r#"<p class="s-failed">not found</p>"#.into(),
            )
        }
    };
    let logs = db.job_logs(&id, LOG_PAGE_LIMIT).unwrap_or_default();
    let mut body = format!(
        r#"<h1>job <code class="id-pill">{id}</code></h1>
<p class="muted">state: <span class="{sc}">{state:?}</span> · priority: <strong>{prio:?}</strong> · submitter: <strong>{sub}</strong></p>"#,
        id = job.id,
        sc = super::state_class(job.state),
        state = job.state,
        prio = job.priority,
        sub = super::html_escape(&job.submitter),
    );
    if let Some(o) = &job.outcome {
        body.push_str(&format!(
            r#"<p>outcome: <code>{}</code></p>"#,
            super::html_escape(&serde_json::to_string(o).unwrap_or_default())
        ));
    }
    body.push_str(r#"<h2>log</h2>"#);
    body.push_str(r#"<pre class="logpane">"#);
    for f in &logs {
        // Render `ts_us` as `mm:ss.fff` (relative to job start) for
        // the visible body. Hover-tooltip resolves to the absolute
        // wall-clock by adding `ts_us` to `submitted_at`. If
        // `ts_us_to_wall_clock` returns `None` (impossible in
        // practice — we always have submitted_at on a real JobRow —
        // we just omit the title attribute, NOT render a broken
        // tooltip).
        let rel = relative_us(f.ts_us, true);
        let tooltip = ts_us_to_wall_clock(f.ts_us, Some(job.submitted_at))
            .map(|abs| format!(r#" title="{}""#, super::html_escape(&abs)))
            .unwrap_or_default();
        // Per-frame level → CSS class so warn/error frames pop without
        // colouring every line. Info/debug/trace stay neutral.
        let lvl_class = match f.level {
            paavo_proto::LogLevel::Error => "lvl-error",
            paavo_proto::LogLevel::Warn => "lvl-warn",
            paavo_proto::LogLevel::Info => "lvl-info",
            paavo_proto::LogLevel::Debug => "lvl-debug",
            paavo_proto::LogLevel::Trace => "lvl-trace",
        };
        body.push_str(&format!(
            "<span class=\"log-line {lvl_class}\"{tooltip}><span class=\"log-ts\">{rel}</span> [{lvl:?}] {msg}</span>\n",
            lvl = f.level,
            msg = super::html_escape(&f.message),
        ));
    }
    body.push_str("</pre>");
    super::html_shell(NavTab::Jobs, "job", body)
}
