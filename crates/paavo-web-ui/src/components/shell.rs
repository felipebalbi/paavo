//! The app shell: a CSS-grid frame of sidebar nav + topbar + main content.
//!
//! `Shell` wraps the routed page (`children`) so the sidebar and topbar persist
//! across client-side navigations — only the `<main>` content swaps. The
//! sidebar links use `leptos_router`'s [`A`], which sets `aria-current="page"`
//! on the active link (the CSS keys the active styling off that attribute —
//! 0.7's `<A>` has no `active_class` prop). The topbar breadcrumb is derived
//! reactively from the current location.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

use crate::theme::ThemeToggle;

/// Map a URL path to a human breadcrumb label.
fn breadcrumb(path: &str) -> &'static str {
    // Match on the first path segment; detail routes fold into their section.
    let seg = path.trim_start_matches('/').split('/').next().unwrap_or("");
    match seg {
        "jobs" => "Jobs",
        "boards" => "Boards",
        "schedule" => "Schedule",
        _ => "Dashboard",
    }
}

/// The persistent shell around every routed page.
#[component]
pub fn Shell(children: Children) -> impl IntoView {
    let location = use_location();
    let crumb = move || breadcrumb(&location.pathname.get());
    view! {
        <div class="app">
            <nav class="sidebar">
                <div class="brand">"paavo"</div>
                // `exact` on the root link so "/" isn't marked active on every route.
                <A href="/" exact=true>"Dashboard"</A>
                <A href="/jobs">"Jobs"</A>
                <A href="/boards">"Boards"</A>
                <A href="/schedule">"Schedule"</A>
            </nav>
            <header class="topbar">
                <span class="breadcrumb">{crumb}</span>
                <ThemeToggle/>
            </header>
            <main>{children()}</main>
        </div>
    }
}
