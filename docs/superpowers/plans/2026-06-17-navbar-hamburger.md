# Responsive Navbar Hamburger Drawer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On screens ≤48rem, replace the horizontal-scrolling nav row with a hamburger button that opens a left slide-in drawer; leave the desktop sidebar unchanged.

**Architecture:** Pure front-end change in the `paavo-web-ui` Leptos CSR crate. The existing `<nav class="sidebar">` is reused as both the desktop sidebar and the mobile drawer — only CSS differs. `Shell` gains one `RwSignal<bool>` (`open`) that toggles a `nav-open` class on `.app`; CSS drives the slide/scrim. Escape-to-close uses the crate's established raw `web_sys` + `Closure::forget()` listener idiom (matching `live.rs`); focus return uses `NodeRef`s + an `Effect`.

**Tech Stack:** Rust + `leptos = 0.7.8` (CSR), `leptos_router = 0.7.8`, `web-sys`, `wasm-bindgen`, built with `trunk`. Pinned toolchain Rust 1.95.0.

---

## Context & ground rules

- This crate is **workspace-excluded** and built by `trunk`, NOT `cargo`. The workspace gates (`cargo fmt/clippy/test --workspace`) do **not** touch it. The build gate here is **`just build-ui`** (`trunk build --release`).
- There are **no automated tests** in this crate today, and the spec deliberately adds none — the change is declarative markup + CSS. Verification is `trunk build` (compiles clean) + a **manual responsive checklist**. This is an approved, intentional deviation from TDD.
- Work happens in the existing worktree: `D:\workspace\paavo\.worktrees\feat-navbar-hamburger` on branch `feat/navbar-hamburger`. All paths below are relative to that worktree root.
- **No AGENTS.md update needed:** this change does not alter crate boundaries, build commands, conventions, or landmines.
- `trunk` and the `wasm32-unknown-unknown` target are already installed; baseline `trunk build` is green.

## File structure

| File | Responsibility | Change |
|------|----------------|--------|
| `crates/paavo-web-ui/Cargo.toml` | wasm deps + enabled `web-sys` features | Add `HtmlElement`, `HtmlButtonElement`, `KeyboardEvent` features |
| `crates/paavo-web-ui/style.css` | All UI styling incl. responsive layout | Add `.topbar-left` / `.nav-toggle` / `.nav-scrim` rules; rewrite the `@media (max-width: 48rem)` block |
| `crates/paavo-web-ui/src/components/shell.rs` | The persistent app shell (sidebar + topbar + main) | Add `open` state, hamburger toggle, scrim, nav aria/id, Escape listener, focus effect |

---

## Task 1: Enable the web-sys features the drawer needs

`KeyboardEvent` (Escape handler) and `HtmlElement`/`HtmlButtonElement` (`.focus()` via `NodeRef`) are not in the current `web-sys` feature set. Add them. (`HtmlButtonElement` transitively enables `HtmlElement`/`Element`, but list all three explicitly to match this file's explicit style.)

**Files:**
- Modify: `crates/paavo-web-ui/Cargo.toml`

- [ ] **Step 1: Add the three features**

In `crates/paavo-web-ui/Cargo.toml`, find the end of the `web-sys` `features = [ … ]` list. It currently ends with:

```toml
    "Document",
    "Element",
    "DomTokenList",
    "MediaQueryList",
] }
```

Replace that with:

```toml
    "Document",
    "Element",
    "DomTokenList",
    "MediaQueryList",
    # Responsive nav drawer (shell.rs): Escape-to-close reads KeyboardEvent.key();
    # focus management calls HtmlElement::focus() on the nav and the toggle
    # button (NodeRef<html::Button> resolves to HtmlButtonElement).
    "HtmlElement",
    "HtmlButtonElement",
    "KeyboardEvent",
] }
```

- [ ] **Step 2: Verify it still builds**

Run (from the worktree root):

```bash
cd crates/paavo-web-ui && trunk build
```

Expected: `✅ success` (unused features are harmless; this just proves the manifest is valid and the tree still compiles).

- [ ] **Step 3: Commit**

```bash
git add crates/paavo-web-ui/Cargo.toml
git commit -m "build(web-ui): enable web-sys features for nav drawer (focus + keydown)"
```

---

## Task 2: Responsive CSS — hamburger, drawer, scrim

Add the base (desktop-hidden) rules for the toggle/scrim/left-cluster, then rewrite the mobile media query so the sidebar becomes an off-canvas drawer instead of a scrolling row.

**Files:**
- Modify: `crates/paavo-web-ui/style.css`

- [ ] **Step 1: Add base rules after the theme-toggle block**

In `crates/paavo-web-ui/style.css`, the theme-toggle block ends at:

```css
.theme-toggle:focus-visible {
  outline: none;
  border-color: var(--accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 25%, transparent);
}
```

Immediately **after** that rule, insert:

```css

/* ---- mobile nav: hamburger toggle + drawer scrim ------------------- */
/* Both are hidden on wide screens; the max-width:48rem block below reveals
 * them and turns .sidebar into a left slide-in drawer. */

/* Left cluster of the topbar: hamburger (mobile only) + breadcrumb. */
.topbar-left {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  min-width: 0;
}

/* Hamburger button. Mirrors .theme-toggle's look; shown only on mobile. */
.nav-toggle {
  display: none; /* flipped to inline-grid in the mobile media query */
  place-items: center;
  width: 2.25rem;
  height: 2.25rem;
  border-radius: 0.6rem;
  border: 1px solid var(--border);
  background: var(--surface-2);
  color: var(--text);
  font-size: 1.1rem;
  line-height: 1;
  cursor: pointer;
  transition: border-color 0.12s ease, background 0.12s ease;
}

.nav-toggle:hover {
  border-color: var(--accent);
}

.nav-toggle:focus-visible {
  outline: none;
  border-color: var(--accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 25%, transparent);
}

/* Dimmed backdrop behind the open drawer. Hidden (and inert) on desktop. */
.nav-scrim {
  display: none;
}
```

- [ ] **Step 2: Rewrite the mobile media query**

Find the existing block (near the end of the file):

```css
@media (max-width: 48rem) {
  .app {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto auto minmax(0, 1fr);
    grid-template-areas:
      "sidebar"
      "topbar"
      "main";
  }

  .sidebar {
    flex-direction: row;
    align-items: center;
    gap: 0.25rem;
    overflow-x: auto;
    padding: 0.5rem 0.75rem;
    border-right: none;
    border-bottom: 1px solid var(--border);
  }

  .brand {
    padding: 0.25rem 0.6rem;
  }

  .sidebar a {
    white-space: nowrap;
  }

  /* Let wide tables scroll horizontally instead of overflowing the viewport. */
  .table {
    display: block;
    overflow-x: auto;
    white-space: nowrap;
  }

  /* Stack the dashboard's two-column content row. */
  .grid2 {
    grid-template-columns: minmax(0, 1fr);
  }
}
```

Replace it **entirely** with:

```css
@media (max-width: 48rem) {
  /* Single column: just the topbar and main. The sidebar leaves normal flow
   * and becomes a left slide-in drawer toggled by `.app.nav-open`. */
  .app {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto minmax(0, 1fr);
    grid-template-areas:
      "topbar"
      "main";
  }

  /* Reveal the hamburger. */
  .nav-toggle {
    display: inline-grid;
  }

  /* Sidebar -> off-canvas drawer. It keeps its column layout, padding, and
   * background from the base `.sidebar` rule; here we only re-position it and
   * slide it in/out. */
  .sidebar {
    position: fixed;
    inset-block: 0;
    left: 0;
    z-index: 20;
    width: clamp(14rem, 75vw, 18rem);
    transform: translateX(-100%);
    transition: transform 0.2s ease;
    overflow-y: auto;
  }

  .app.nav-open .sidebar {
    transform: translateX(0);
  }

  /* Backdrop: covers the viewport, fades in with the drawer, and is
   * click-through until the drawer is open. */
  .nav-scrim {
    display: block;
    position: fixed;
    inset: 0;
    z-index: 10;
    background: rgba(0, 0, 0, 0.4);
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.2s ease;
  }

  .app.nav-open .nav-scrim {
    opacity: 1;
    pointer-events: auto;
  }

  /* Let wide tables scroll horizontally instead of overflowing the viewport. */
  .table {
    display: block;
    overflow-x: auto;
    white-space: nowrap;
  }

  /* Stack the dashboard's two-column content row. */
  .grid2 {
    grid-template-columns: minmax(0, 1fr);
  }
}
```

Note: `prefers-reduced-motion` already nulls all transitions globally (`@media (prefers-reduced-motion: reduce) { * { transition: none !important } }`), so the drawer/scrim simply snap rather than animate under that setting — no extra rule needed.

- [ ] **Step 3: Verify the build (CSS only; markup lands in Task 3)**

```bash
cd crates/paavo-web-ui && trunk build
```

Expected: `✅ success`. (No visible behavior change yet — the markup classes `nav-toggle`/`nav-scrim`/`topbar-left`/`nav-open` don't exist in the DOM until Task 3.)

- [ ] **Step 4: Commit**

```bash
git add crates/paavo-web-ui/style.css
git commit -m "style(web-ui): drawer + hamburger + scrim CSS; drop horizontal nav scroll"
```

---

## Task 3: Shell markup, state, Escape, and focus

Rewrite `Shell` to add the `open` signal, the hamburger button, the scrim, nav aria/id, the Escape listener, and focus return. This is a single coherent file rewrite; the steps below build it up and verify each behavior.

**Files:**
- Modify: `crates/paavo-web-ui/src/components/shell.rs`

- [ ] **Step 1: Replace the file contents**

Write `crates/paavo-web-ui/src/components/shell.rs` with exactly:

```rust
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
    // be moved into the keydown closure and the focus effect freely.
    let open = RwSignal::new(false);

    // Node handles for focus management.
    let toggle_ref: NodeRef<html::Button> = NodeRef::new();
    let nav_ref: NodeRef<html::Nav> = NodeRef::new();

    // Escape closes the drawer. We register a raw `keydown` listener on `window`
    // and `forget()` the closure — the same app-lifetime-listener idiom used in
    // `live.rs` for the `EventSource`. `Shell` mounts once at the root, so this
    // leaks exactly one closure, intentionally and boundedly. Setting `open`
    // when it is already `false` is a harmless no-op, so we don't read it first.
    {
        let cb = Closure::<dyn FnMut(KeyboardEvent)>::new(move |ev: KeyboardEvent| {
            if ev.key() == "Escape" {
                open.set(false);
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
                if let Some(nav) = nav_ref.get() {
                    let _ = nav.focus();
                }
            } else if let Some(btn) = toggle_ref.get() {
                let _ = btn.focus();
            }
        }
        is_open
    });

    view! {
        <div class="app" class=("nav-open", move || open.get())>
            <nav
                class="sidebar"
                id="primary-nav"
                aria-label="Primary"
                tabindex="-1"
                node_ref=nav_ref
                // Any tap inside the drawer (e.g. a nav link) dismisses it.
                on:click=move |_| open.set(false)
            >
                <div class="brand">"paavo"</div>
                // `exact` on the root link so "/" isn't marked active on every route.
                <A href="/" exact=true>"Dashboard"</A>
                <A href="/jobs">"Jobs"</A>
                <A href="/boards">"Boards"</A>
                <A href="/schedule">"Schedule"</A>
            </nav>

            // Backdrop behind the open drawer (mobile only; CSS hides it on desktop).
            <div class="nav-scrim" on:click=move |_| open.set(false)></div>

            <header class="topbar">
                <div class="topbar-left">
                    <button
                        class="nav-toggle"
                        type="button"
                        node_ref=toggle_ref
                        aria-controls="primary-nav"
                        aria-expanded=move || if open.get() { "true" } else { "false" }
                        aria-label=move || {
                            if open.get() {
                                "Close navigation"
                            } else {
                                "Open navigation"
                            }
                        }
                        title=move || {
                            if open.get() {
                                "Close navigation"
                            } else {
                                "Open navigation"
                            }
                        }
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
```

- [ ] **Step 2: Format the crate**

```bash
cd crates/paavo-web-ui && cargo fmt
```

Expected: no error. (The workspace `cargo fmt --all` skips this excluded crate, so format it directly.)

- [ ] **Step 3: Build**

```bash
cd crates/paavo-web-ui && trunk build
```

Expected: `✅ success` with no warnings. If a `web-sys` method is reported missing, re-check Task 1's features.

- [ ] **Step 4: Manual verification — serve the UI**

From the worktree root:

```bash
just web
```

This runs `trunk build --release` then serves `paavo-web` at `http://127.0.0.1:8081` (per `sample-paavo.toml`). Open it in a browser. (If you want live data, also run a daemon with `PAAVO_FAKE_RUNNER=1`, but it is not required to exercise the nav.)

- [ ] **Step 5: Manual checklist — desktop (wide window, >48rem)**

Confirm:
- Sidebar shows on the left with brand + 4 links; topbar shows breadcrumb + theme toggle.
- **No** hamburger button is visible.
- Layout is unchanged from before this work.

- [ ] **Step 6: Manual checklist — mobile (narrow the window to <48rem, e.g. DevTools device toolbar / ~375px)**

Confirm:
- The sidebar is gone; the topbar shows `☰` on the left, breadcrumb, and the theme toggle on the right. There is **no** horizontal-scrolling nav row.
- Tapping `☰` slides a drawer in from the left over a dimmed scrim; the glyph becomes `✕`.
- Each close path works:
  - tapping a nav link navigates **and** closes the drawer;
  - tapping the dimmed scrim closes it;
  - pressing `Escape` closes it;
  - tapping `✕` closes it.
- `aria-expanded` flips true/false (Inspect the button) and after closing via Escape, keyboard focus is back on the `☰` button.
- The theme toggle still works and stays in the topbar.

- [ ] **Step 7: Manual checklist — reduced motion**

Enable "Reduce motion" (OS setting, or DevTools → Rendering → Emulate CSS `prefers-reduced-motion: reduce`). Confirm the drawer **snaps** open/closed with no slide, and still functions.

- [ ] **Step 8: Final release build gate**

```bash
just build-ui
```

Expected: `trunk build --release` → `✅ success`, no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/paavo-web-ui/src/components/shell.rs
git commit -m "feat(web-ui): collapse navbar to a hamburger drawer on small screens"
```

---

## Self-review notes (author)

- **Spec coverage:** left drawer (Task 2/3) ✓; single 48rem breakpoint (Task 2) ✓; four dismissal paths — link tap via nav `on:click`, scrim tap, Escape listener, toggle button (Task 3) ✓; mobile top bar `[☰][breadcrumb][☾]` (Task 3 markup + `.topbar-left` CSS) ✓; brand in drawer (nav contains `.brand`) ✓; aria-expanded/controls/label (Task 3) ✓; focus into drawer / back to toggle (Task 3 effect) ✓; reduced-motion (existing global rule; verified Step 7) ✓; build + manual verification (Task 3 Steps 4-8) ✓.
- **Type consistency:** `open: RwSignal<bool>` used consistently (`.get()`, `.set()`, `.update()`); `toggle_ref: NodeRef<html::Button>` → `HtmlButtonElement::focus()`; `nav_ref: NodeRef<html::Nav>` → `HtmlElement::focus()`; features added in Task 1 cover both plus `KeyboardEvent`.
- **No placeholders:** every code block is complete and final.
- **Hyphenated class:** uses `class=("nav-open", …)` tuple form (the `class:ident` directive can't express a hyphen).
