# Responsive navbar: collapse to a hamburger drawer on small screens

**Date:** 2026-06-17
**Status:** Approved (design)
**Crate:** `paavo-web-ui` (workspace-excluded Leptos CSR SPA, built by `trunk`)

## Problem

On the web UI, the primary navigation is a left sidebar (`.sidebar`: brand +
four links — Dashboard, Jobs, Boards, Schedule) on wide screens. At
`max-width: 48rem` the CSS reflows the sidebar into a **horizontal row** above
the topbar with `overflow-x: auto` (`style.css:572-598`). On phones this row
scrolls horizontally, which is a clunky, easy-to-miss interaction. We want the
nav to collapse to a hamburger button that opens a slide-in drawer instead.

## Goals

- Below a single breakpoint, replace the horizontal-scroll nav row with a
  hamburger button that toggles a left slide-in drawer.
- Keep the desktop sidebar layout unchanged.
- No changes to routing, theming, or any page/route component.

## Non-goals (YAGNI)

- No full focus-trap inside the drawer (four links; basic focus handling only).
- No body-scroll lock while the drawer is open (the scrim covers content).
- No third "tablet" layout tier.
- No new automated tests (this crate has none today; the change is declarative
  markup + CSS — a wasm-bindgen-test would exercise the framework, not our
  logic).

## Decisions

| Question | Decision |
|----------|----------|
| Collapse pattern | **Left slide-in drawer** over dimmed content (mirrors the desktop sidebar's left edge). |
| Breakpoint | **Single breakpoint at `48rem`.** Above → desktop sidebar; at/below → hamburger drawer. The intermediate horizontal-scroll row is removed entirely. |
| Dismissal | Close on: **link tap**, **scrim (backdrop) tap**, **Escape**, and the **toggle button** (☰ ⇄ ✕). |
| Mobile top bar | `[☰] [breadcrumb] ........ [☾ theme toggle]`. |
| Brand placement | The brand "paavo" stays inside `<nav>`, so it serves as the drawer header on mobile. |

## Design

### Structure & state — `crates/paavo-web-ui/src/components/shell.rs`

`Shell` gains one reactive flag: `let open = RwSignal::new(false);` (matches the
`RwSignal` pattern in `theme.rs`). A `close` helper sets it to `false`.

```
<div class="app" class:nav-open=move || open.get()>
  <nav class="sidebar" id="primary-nav" aria-label="Primary">
     <div class="brand">"paavo"</div>
     // each <A> link gets on:click that calls close()
     <A href="/" exact=true>"Dashboard"</A> ... etc.
  </nav>

  <div class="nav-scrim" on:click=close></div>   // backdrop; mobile-only via CSS

  <header class="topbar">
     <div class="topbar-left">
        <button class="nav-toggle" ...>{☰ / ✕}</button>   // mobile-only via CSS
        <span class="breadcrumb">{crumb}</span>
     </div>
     <ThemeToggle/>
  </header>

  <main>{children()}</main>
</div>
```

Key points:

- The **same `<nav class="sidebar">`** is the desktop sidebar and the mobile
  drawer; only CSS differs. No duplicated markup.
- The toggle button and scrim exist in the DOM always but are `display:none`
  above the breakpoint.

### Behavior & accessibility

- **Toggle button:** `aria-controls="primary-nav"`, `aria-expanded` bound to
  `open`, and an `aria-label`/`title` that flips between
  "Open navigation" / "Close navigation". The glyph flips ☰ → ✕ while open.
- **Escape to close:** a single `window_event_listener(ev::keydown, …)`
  registered once in `Shell`; it calls `close()` only when `open` is set. (The
  `Shell` is persistent for the app's lifetime, so a single registration is
  fine.)
- **Focus:** on open, move focus into the drawer; on close, return focus to the
  toggle button. Implemented with `NodeRef`s on the nav and the toggle. No full
  focus-trap.

### Responsive CSS — `crates/paavo-web-ui/style.css`

- **Desktop (>48rem):** existing `.app` grid unchanged. `.nav-toggle` and
  `.nav-scrim` are `display:none`. `.topbar-left` is a simple flex cluster
  (breadcrumb only is visible).
- **Mobile (≤48rem):** rewrite the existing `@media (max-width: 48rem)` block:
  - Grid drops to two rows: `"topbar"` / `"main"` (the sidebar leaves normal
    flow).
  - `.sidebar` becomes off-canvas: `position:fixed; inset-block:0; left:0;
    width:clamp(14rem,75vw,18rem); transform:translateX(-100%);
    transition:transform; z-index above content`. It keeps its vertical
    `flex-direction:column` layout (like desktop).
  - `.app.nav-open .sidebar` → `transform:translateX(0)`.
  - `.nav-scrim` → `display:block; position:fixed; inset:0;
    background:rgba(0,0,0,.4); opacity:0; pointer-events:none;
    transition:opacity`. `.app.nav-open .nav-scrim` → `opacity:1;
    pointer-events:auto`.
  - `.nav-toggle` → visible (styled like `.theme-toggle`).
  - **Remove** the old `flex-direction:row; overflow-x:auto` sidebar rules —
    this deletes the clunky horizontal scroll.
- `prefers-reduced-motion` already disables all transitions globally
  (`style.css:615-622`), so under that setting the drawer snaps open/closed with
  no extra code.

## Testing / verification

- `just build-ui` (`trunk build --release`) compiles cleanly — the build gate
  for this wasm crate. (Baseline `trunk build` confirmed green before changes.)
- Manual responsive check at <48rem:
  - hamburger appears; the horizontal-scroll row is gone;
  - drawer slides in from the left over a dimmed scrim;
  - each of the four close paths works (link tap, scrim tap, Escape, toggle);
  - `aria-expanded` toggles; focus returns to the toggle on close;
  - theme toggle remains reachable in the top bar;
  - **desktop layout (>48rem) is unchanged**;
  - with reduced-motion enabled, the drawer snaps instead of sliding.

## Affected files

- `crates/paavo-web-ui/src/components/shell.rs` — markup, `open` state, toggle
  button, scrim, Escape listener, focus handling.
- `crates/paavo-web-ui/style.css` — `.nav-toggle`, `.nav-scrim`, `.topbar-left`,
  and the rewritten `@media (max-width: 48rem)` block.
- Possibly `crates/paavo-web-ui/Cargo.toml` — add any `web-sys` feature needed
  for `KeyboardEvent` / element `focus()` if not already enabled.
