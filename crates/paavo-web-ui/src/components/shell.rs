//! The app shell: a CSS-grid frame of sidebar nav + topbar + main content.
//!
//! `Shell` wraps the routed page (`children`) so the sidebar and topbar persist
//! across client-side navigations — only the `<main>` content swaps. The
//! sidebar links use `leptos_router`'s [`A`], which sets `aria-current="page"`
//! on the active link (the CSS keys the active styling off that attribute —
//! 0.7's `<A>` has no `active_class` prop). The topbar breadcrumb is derived
//! reactively from the current location.
//!
//! Responsive nav: on wide screens the `<nav class="sidebar">` is a fixed left
//! column. At `max-width: 48rem` the CSS turns the very same `<nav>` into a
//! left slide-in drawer; a hamburger button in the topbar toggles the `open`
//! signal, which flips a `nav-open` class on `.app`. The drawer is dismissed by
//! tapping a link (any click inside the nav), tapping the scrim, pressing
//! Escape, or pressing the toggle again. Focus moves into the drawer on open
//! and returns to the toggle on close.

use leptos::html;
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent;

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

    // Drawer open/closed state. Only meaningful at <=48rem; on wide screens the
    // CSS ignores the `nav-open` class entirely. `RwSignal` is `Copy`, so it can
    // be moved into the keydown closure, the close helper, and the focus effect.
    let open = RwSignal::new(false);

    // Close the drawer, but only when it is actually open. Plain signals notify
    // on *every* `set`, so an unconditional `open.set(false)` on an
    // already-closed drawer would still re-run the focus effect below and yank
    // focus to the toggle (e.g. pressing Escape when nothing is open, or
    // clicking a sidebar link on desktop). Gating on the current value makes the
    // close a true no-op when already closed.
    let close = move || {
        if open.get_untracked() {
            open.set(false);
        }
    };

    // Node handles for focus management.
    let toggle_ref: NodeRef<html::Button> = NodeRef::new();
    let nav_ref: NodeRef<html::Nav> = NodeRef::new();

    // Escape closes the drawer. We register a raw `keydown` listener on `window`
    // and `forget()` the closure — the same app-lifetime-listener idiom used in
    // `live.rs` for the `EventSource`. `Shell` mounts once at the root, so this
    // leaks exactly one closure, intentionally and boundedly.
    {
        let cb = Closure::<dyn FnMut(KeyboardEvent)>::new(move |ev: KeyboardEvent| {
            if ev.key() == "Escape" {
                close();
            }
        });
        if let Some(win) = web_sys::window() {
            let _ = win.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
        }
        cb.forget();
    }

    // Move focus into the drawer when it opens; restore focus to the toggle when
    // it closes. The `prev` argument is `None` on the effect's first run, which
    // we skip so we don't steal focus on initial paint.
    Effect::new(move |prev: Option<bool>| {
        let is_open = open.get();
        if prev.is_some() {
            if is_open {
                // The drawer is `visibility: hidden` until the `nav-open` class
                // lands on `.app` (a separate render effect whose ordering
                // relative to this one isn't guaranteed). Defer the focus to the
                // next animation frame so the nav is actually focusable when we
                // call `focus()`.
                request_animation_frame(move || {
                    if let Some(nav) = nav_ref.get() {
                        let _ = nav.focus();
                    }
                });
            } else if let Some(btn) = toggle_ref.get() {
                let _ = btn.focus();
            }
        }
        is_open
    });

    // One reactive label drives both the accessible name and the tooltip of the
    // toggle (mirrors `theme.rs`).
    let toggle_label = move || {
        if open.get() {
            "Close navigation"
        } else {
            "Open navigation"
        }
    };

    view! {
        <div class="app" class=("nav-open", move || open.get())>
            <nav
                class="sidebar"
                id="primary-nav"
                aria-label="Primary"
                tabindex="-1"
                node_ref=nav_ref
                // Any tap inside the drawer (e.g. a nav link) dismisses it.
                on:click=move |_| close()
            >
                <div class="brand">"paavo"</div>
                // `exact` on the root link so "/" isn't marked active on every route.
                <A href="/" exact=true>"Dashboard"</A>
                <A href="/jobs">"Jobs"</A>
                <A href="/boards">"Boards"</A>
                <A href="/schedule">"Schedule"</A>
            </nav>

            // Backdrop behind the open drawer (mobile only; CSS hides it on desktop).
            <div class="nav-scrim" on:click=move |_| close()></div>

            <header class="topbar">
                <div class="topbar-left">
                    <button
                        class="nav-toggle"
                        type="button"
                        node_ref=toggle_ref
                        aria-controls="primary-nav"
                        aria-expanded=move || if open.get() { "true" } else { "false" }
                        aria-label=toggle_label
                        title=toggle_label
                        on:click=move |_| open.update(|o| *o = !*o)
                    >
                        {move || if open.get() { "✕" } else { "☰" }}
                    </button>
                    <span class="breadcrumb">{crumb}</span>
                </div>
                <ThemeToggle/>
            </header>

            <main>{children()}</main>
        </div>
    }
}
