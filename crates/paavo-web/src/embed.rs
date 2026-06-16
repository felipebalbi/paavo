//! Embedded WASM UI assets + SPA fallback.
//!
//! The browser-side app (`paavo-web-ui`, a separate `trunk`-built
//! crate) compiles to a `dist/` bundle: an `index.html`, a JS loader,
//! a `*_bg.wasm`, and a hashed stylesheet. `rust-embed` bakes that
//! directory into the `paavo-web` binary at compile time so the daemon
//! stays a single-file deploy (no sibling `dist/` to ship).
//!
//! Routing contract:
//! - a request whose path matches an embedded asset serves that asset
//!   with a best-effort content type;
//! - anything else falls back to `index.html`, because the SPA owns
//!   client-side routing (`/jobs/:id` etc. are virtual routes the WASM
//!   resolves, not server paths);
//! - if the UI was never built (`dist/` absent at compile time, e.g. a
//!   backend-only checkout), `#[allow_missing]` lets the crate still
//!   compile and every request falls through to a placeholder that
//!   explains how to build it.
//!
//! `dist/` is git-ignored and produced out of band (`just build-ui` /
//! `trunk build`), so the embedded set reflects whatever was present
//! when `paavo-web` was compiled.
use axum::http::{header, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};

/// Compile-time snapshot of `paavo-web-ui/dist`. The path is relative
/// to this crate's `CARGO_MANIFEST_DIR` (`crates/paavo-web`).
///
/// `allow_missing` keeps `paavo-web` compilable on a fresh checkout/CI
/// where `dist/` has not been produced yet: rust-embed normally hard-
/// errors on an absent `#[folder]`. With it absent the generated asset
/// set is empty and [`serve`] returns the not-built placeholder; a
/// later `just build-ui` populates the real bundle.
#[derive(rust_embed::RustEmbed)]
#[folder = "../paavo-web-ui/dist"]
#[allow_missing = true]
struct Assets;

/// Shown when no `index.html` was embedded (the UI bundle was never
/// built into `dist/` before compiling `paavo-web`).
const NOT_BUILT: &str = "<h1>paavo-web UI not built</h1><p>Run <code>just build-ui</code>.</p>";

/// Serve an embedded asset by request path; otherwise fall back to the
/// SPA shell (`index.html`) so client-side routes resolve; otherwise a
/// not-built placeholder.
pub async fn serve(uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };
    if let Some(f) = Assets::get(path) {
        return (
            [(header::CONTENT_TYPE, mime_for(path))],
            f.data.into_owned(),
        )
            .into_response();
    }
    match Assets::get("index.html") {
        Some(f) => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            f.data.into_owned(),
        )
            .into_response(),
        None => (StatusCode::OK, Html(NOT_BUILT)).into_response(),
    }
}

/// Best-effort content type from a file extension. Deliberately tiny:
/// the `trunk` bundle only emits html/js/wasm/css; anything else is an
/// opaque download.
fn mime_for(p: &str) -> &'static str {
    if p.ends_with(".wasm") {
        "application/wasm"
    } else if p.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if p.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if p.ends_with(".html") {
        "text/html; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}
