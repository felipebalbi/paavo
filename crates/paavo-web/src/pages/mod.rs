//! HTML pages.

pub mod boards;
pub mod dashboard;
pub mod job_detail;
pub mod jobs_list;
pub mod schedule;

use axum::response::Html;

/// HTML shell wrapping a page body. Pulls in the UnoCSS CDN runtime so
/// utility classes work without a build step. Mono font + zinc palette
/// throughout for the "techy + clean + easy-to-read" feel.
pub fn html_shell(title: &str, body: String) -> Html<String> {
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>{title} — paavo</title>
<script src="https://cdn.jsdelivr.net/npm/@unocss/runtime"></script>
<script>
  // Pin UnoCSS preset before the runtime applies. Defaults are fine for v1.
  window.__unocss = {{
    presets: [() => window.__unocss_runtime?.presets.uno()],
  }};
</script>
</head>
<body class="font-mono text-zinc-900 bg-zinc-50 leading-relaxed">
<nav class="sticky top-0 backdrop-blur bg-zinc-50/80 border-b border-zinc-200 px-6 py-3 flex gap-6">
  <a href="/" class="hover:text-blue-700">dashboard</a>
  <a href="/jobs" class="hover:text-blue-700">jobs</a>
  <a href="/boards" class="hover:text-blue-700">boards</a>
  <a href="/schedule" class="hover:text-blue-700">schedule</a>
</nav>
<main class="max-w-5xl mx-auto p-6">
{body}
</main>
</body>
</html>"#
    ))
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

/// Map a `JobState` to its UnoCSS color class. Used by every page that
/// displays a state badge or a state column.
pub(crate) fn state_class(s: paavo_proto::JobState) -> &'static str {
    use paavo_proto::JobState::*;
    match s {
        Passed => "text-emerald-700",
        Failed | TimedOut | Aborted => "text-rose-700",
        Running | Building => "text-blue-700",
        Submitted => "text-zinc-600",
    }
}

/// Map a `BoardHealth` to its UnoCSS color class.
pub(crate) fn health_class(h: paavo_proto::BoardHealth) -> &'static str {
    match h {
        paavo_proto::BoardHealth::Healthy => "text-emerald-700",
        paavo_proto::BoardHealth::Quarantined => "text-rose-700",
    }
}
