//! HTML pages.

pub mod boards;
pub mod dashboard;
pub mod job_detail;
pub mod jobs_list;
pub mod schedule;

use axum::response::Html;

/// Cargo package version, baked at compile time. Used as a cache-bust
/// query param on the `/static/style.css` link so a paavo-web rebuild
/// (almost always = new release) forces clients off any stale CSS
/// without waiting for the browser's HTTP cache TTL to expire.
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Identifier for the page so the nav can mark the current entry with
/// `aria-current="page"` and a CSS rule can colour it. Pure-data, no
/// fragility — pages call `html_shell(NavTab::Jobs, body)` once and
/// every nav item gets the right styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavTab {
    /// `/`
    Dashboard,
    /// `/jobs`, `/jobs/:id`
    Jobs,
    /// `/boards`
    Boards,
    /// `/schedule`
    Schedule,
}

/// HTML shell wrapping a page body. Links the baked-in
/// `/static/style.css` (ef-cyprus light + ef-symbiosis dark, see
/// `crates/paavo-web/src/assets/style.css`) and marks `tab` as the
/// current page in the top nav.
///
/// The previous shell pulled UnoCSS via a CDN script tag at every
/// page load. That worked for development but is fragile in three
/// ways: (1) air-gapped deployments break, (2) every page hits a
/// third-party host, (3) flash-of-unstyled-content is visible while
/// the runtime parses utility classes. Baking a static stylesheet
/// fixes all three; the CSS file is `include_str!`-ed into the
/// binary so the deploy story stays "one binary".
pub fn html_shell(tab: NavTab, title: &str, body: String) -> Html<String> {
    let nav = render_nav(tab);
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<meta name="color-scheme" content="light dark"/>
<title>{title} — paavo</title>
<link rel="stylesheet" href="/static/style.css?v={PKG_VERSION}"/>
</head>
<body>
{nav}
<main>
{body}
</main>
</body>
</html>"#
    ))
}

/// Render the top-of-page navigation, marking `current` with
/// `aria-current="page"` so screen readers and the matching CSS rule
/// pick it up. Order is dashboard → jobs → boards → schedule, matching
/// the operator's typical reading order ("everything → recent activity
/// → board fleet → cron").
fn render_nav(current: NavTab) -> String {
    let item = |t: NavTab, href: &str, label: &str| -> String {
        let aria = if t == current {
            r#" aria-current="page""#
        } else {
            ""
        };
        format!(r#"<a href="{href}"{aria}>{label}</a>"#)
    };
    format!(
        r#"<nav class="top">
{}
{}
{}
{}
</nav>"#,
        item(NavTab::Dashboard, "/", "dashboard"),
        item(NavTab::Jobs, "/jobs", "jobs"),
        item(NavTab::Boards, "/boards", "boards"),
        item(NavTab::Schedule, "/schedule", "schedule"),
    )
}

/// HTML-escape `s` for safe interpolation into element text **and**
/// attribute values. Covers all five characters per the OWASP XSS
/// Prevention Cheat Sheet's "Rule #1" in a single pass.
pub(crate) fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            other => out.push(other),
        }
    }
    out
}

/// Map a `JobState` to its semantic CSS class (declared in
/// `assets/style.css`). The class names are kebab-case prefixed `s-`
/// so they sort visually together in the stylesheet and are easy to
/// grep.
pub(crate) fn state_class(s: paavo_proto::JobState) -> &'static str {
    use paavo_proto::JobState::*;
    match s {
        Passed => "s-passed",
        Failed => "s-failed",
        TimedOut => "s-timedout",
        Aborted => "s-aborted",
        Running => "s-running",
        Building => "s-building",
        AwaitingBoard => "s-awaiting_board",
        Submitted => "s-submitted",
    }
}

/// Map a `BoardHealth` to its semantic CSS class.
pub(crate) fn health_class(h: paavo_proto::BoardHealth) -> &'static str {
    match h {
        paavo_proto::BoardHealth::Healthy => "health-healthy",
        paavo_proto::BoardHealth::Quarantined => "health-quarantined",
    }
}
