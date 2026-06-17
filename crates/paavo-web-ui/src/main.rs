//! Binary entry point: install the panic hook (so Rust panics surface in the
//! browser console with a backtrace) and mount the [`App`] root onto `<body>`.
//! All actual UI lives in the library crate (`paavo_web_ui`).
//!
//! [`App`]: paavo_web_ui::App

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(paavo_web_ui::App);
}
