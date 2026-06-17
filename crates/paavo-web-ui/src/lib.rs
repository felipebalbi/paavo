//! paavo-web-ui — the Leptos client-side-rendered (CSR) single-page app.
//!
//! This crate compiles to `wasm32-unknown-unknown` and is built by `trunk`
//! (not cargo; it is excluded from the workspace). The backend, `paavo-web`,
//! embeds the resulting `dist/` bundle via `rust-embed` and serves it; the
//! browser boots the wasm, fetches JSON from the same-origin `/api/...`
//! endpoints, subscribes to `/api/events` for live revision bumps, and renders
//! every view client-side.
//!
//! Module layout:
//! - [`app`] — the root [`App`] component: theme bootstrap, live-signal
//!   context, and the `leptos_router` route table wrapped in the [`Shell`].
//! - [`api`] — typed `fetch` wrappers returning `paavo-proto` wire types.
//! - [`live`] — one `EventSource` over `/api/events` exposing per-resource
//!   revision signals that drive refetches.
//! - [`theme`] — light/dark theme read/apply/toggle plus the sun/moon button.
//! - [`components`] — the app shell and one component per route page.
//!
//! [`Shell`]: components::shell::Shell

pub mod api;
pub mod app;
pub mod components;
pub mod live;
pub mod theme;

pub use app::App;
