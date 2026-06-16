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

/// Cargo package version (for the cache-bust on `/static/live-log.js`).
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

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

    // Header card: id, state, priority, submitter.
    let mut body = format!(
        r#"<h1>job <code class="id-pill">{id}</code></h1>
<p class="muted">state: <span class="{sc}">{state:?}</span> · priority: <strong>{prio:?}</strong> · submitter: <strong>{sub}</strong></p>"#,
        id = job.id,
        sc = super::state_class(job.state),
        state = job.state,
        prio = job.priority,
        sub = super::html_escape(&job.submitter),
    );

    // Phase banner. Initial label depends on the current job state:
    // - terminal states → "done" (the banner's grey style)
    // - Running → "running"
    // - Building → "building"
    // - Submitted → "submitted" (live JS will overwrite when the job
    //   actually starts; a subscriber that joined while still
    //   Submitted sees this stable initial label).
    // The classes match `assets/style.css`'s `.phase-banner.{build,run,done}`
    // declarations; `.submitted` falls through to the default styling.
    let (banner_class, banner_text) = match job.state {
        paavo_proto::JobState::Building => ("build", "phase: building"),
        paavo_proto::JobState::Running => ("run", "phase: running"),
        paavo_proto::JobState::Submitted => ("", "phase: submitted"),
        _ => ("done", "phase: done"),
    };
    body.push_str(&format!(
        r#"<p style="margin: 0.6rem 0 1rem"><span id="phase-banner" class="phase-banner {banner_class}">{banner_text}</span> <span id="stream-status" class="muted" style="margin-left: 0.6rem"></span></p>"#
    ));

    // Outcome card. Hidden during in-flight jobs; revealed by the
    // live JS when a `terminal` event arrives. For terminal jobs
    // already in the DB, render visible immediately.
    let outcome_hidden = if job.outcome.is_some() { "" } else { "hidden" };
    let outcome_json = match &job.outcome {
        Some(o) => serde_json::to_string_pretty(o).unwrap_or_default(),
        None => String::new(),
    };
    body.push_str(&format!(
        r#"<div id="outcome-card" {outcome_hidden}><h2>outcome</h2><pre id="outcome-json" class="logpane" style="white-space: pre-wrap">{outcome}</pre></div>"#,
        outcome = super::html_escape(&outcome_json),
    ));

    // Log section.
    body.push_str(r#"<h2>log</h2>"#);
    // The pane carries data-job-id so live-log.js can find which job
    // to subscribe to. data-since-seq is the max seq of the
    // SSR-rendered frames: live-log.js seeds its dedup cursor from it
    // and passes it to the stream so the proxy trims the prefix the
    // page already shows. Omitted when there are no historical rows.
    // Pure DOM — no inline JS, keeps CSP-friendly.
    let since_seq_attr = logs
        .iter()
        .map(|f| f.seq)
        .max()
        .map(|m| format!(r#" data-since-seq="{m}""#))
        .unwrap_or_default();
    body.push_str(&format!(
        r#"<pre id="logpane" class="logpane" data-job-id="{id}"{since}>"#,
        id = super::html_escape(&job.id.to_string()),
        since = since_seq_attr,
    ));
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
        // Phase tag inferred from `target`: `cargo:*` → build,
        // anything else (or absent) → run. Matches the proxy's
        // `current_phase` enrichment so historical and live frames
        // visually align.
        let phase_class = match f.target.as_deref() {
            Some(t) if t.starts_with("cargo:") => "phase-build",
            _ => "phase-run",
        };
        let phase_tag = match f.target.as_deref() {
            Some(t) if t.starts_with("cargo:") => "[build]\u{a0}",
            _ => "[run]\u{a0}",
        };
        body.push_str(&format!(
            "<span class=\"log-line {lvl_class}\"{tooltip}><span class=\"{phase_class}\">{phase_tag}</span><span class=\"log-ts\">{rel}</span> [{lvl:?}] {msg}</span>\n",
            lvl = f.level,
            msg = super::html_escape(&f.message),
        ));
    }
    body.push_str("</pre>");

    // Live tail script. Loaded after the pane so the JS can find
    // #logpane synchronously. Cache-busted via the package version
    // for the same reasons /static/style.css is.
    body.push_str(&format!(
        r#"<script src="/static/live-log.js?v={PKG_VERSION}"></script>"#
    ));

    super::html_shell(NavTab::Jobs, "job", body)
}
