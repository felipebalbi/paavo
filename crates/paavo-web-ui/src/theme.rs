//! Light/dark theme: read, apply, toggle, and the sun/moon toggle button.
//!
//! The active theme is a single `dark` class on `<html>`
//! (`document.documentElement`); `style.css` keys both palettes off `:root`
//! (light) and `:root.dark` (dark), so adding/removing that one class is the
//! entire switch. The choice is persisted in `localStorage["paavo-theme"]`;
//! absent a stored choice we follow the OS `prefers-color-scheme`.

use leptos::prelude::*;

/// `localStorage` key holding the persisted theme choice.
const KEY: &str = "paavo-theme";

/// One of the two themes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// Light palette (`:root`).
    Light,
    /// Dark palette (`:root.dark`).
    Dark,
}

impl Theme {
    /// The string persisted in `localStorage`.
    fn as_str(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    /// The other theme.
    fn flipped(self) -> Theme {
        match self {
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::Light,
        }
    }
}

/// The effective current theme: the persisted choice if any, else the OS
/// `prefers-color-scheme`, else light.
pub fn current() -> Theme {
    if let Some(stored) = storage().and_then(|s| s.get_item(KEY).ok().flatten()) {
        return if stored == "dark" {
            Theme::Dark
        } else {
            Theme::Light
        };
    }
    if prefers_dark() {
        Theme::Dark
    } else {
        Theme::Light
    }
}

/// Apply `theme` by toggling the `dark` class on `<html>`. Idempotent; safe to
/// call repeatedly.
pub fn apply(theme: Theme) {
    if let Some(root) = document_element() {
        let list = root.class_list();
        let _ = match theme {
            Theme::Dark => list.add_1("dark"),
            Theme::Light => list.remove_1("dark"),
        };
    }
}

/// Flip the theme, apply it, persist the choice, and return the new theme.
pub fn toggle() -> Theme {
    let next = current().flipped();
    apply(next);
    if let Some(s) = storage() {
        let _ = s.set_item(KEY, next.as_str());
    }
    next
}

/// `window.localStorage`, if available (absent in some privacy modes).
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// `document.documentElement` (the `<html>` element).
fn document_element() -> Option<web_sys::Element> {
    web_sys::window()?.document()?.document_element()
}

/// Whether the OS currently prefers a dark colour scheme.
fn prefers_dark() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        .map(|m| m.matches())
        .unwrap_or(false)
}

/// Sun/moon button (top-right of the topbar) that flips light/dark on click.
/// It shows the glyph for the theme you would switch *to* — a sun while dark,
/// a moon while light — which is the conventional affordance.
#[component]
pub fn ThemeToggle() -> impl IntoView {
    // Track the current theme reactively so the glyph/label update on click.
    let theme = RwSignal::new(current());
    let on_click = move |_| {
        theme.set(toggle());
    };
    let glyph = move || match theme.get() {
        Theme::Dark => "☀",
        Theme::Light => "☾",
    };
    let label = move || match theme.get() {
        Theme::Dark => "Switch to light theme",
        Theme::Light => "Switch to dark theme",
    };
    view! {
        <button
            class="theme-toggle"
            type="button"
            on:click=on_click
            aria-label=label
            title=label
        >
            {glyph}
        </button>
    }
}
