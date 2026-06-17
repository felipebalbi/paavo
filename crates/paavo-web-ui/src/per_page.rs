//! Per-list "rows per page" preference: the offered sizes, the default, and
//! browser-local (per-user) persistence.
//!
//! Mirrors [`crate::theme`]'s localStorage pattern: a `storage()` guard that
//! yields `None` in privacy modes, default-on-absent, and best-effort
//! persist-on-change. The chosen size is a UI preference only — it is never
//! sent to or stored by the server.

/// The page sizes offered in the selector, in display order. The selector
/// ([`crate::components::widgets::per_page_selector`]) and the validation in
/// [`sanitize`] both key off this single list.
pub const OPTIONS: [u32; 6] = [10, 20, 30, 40, 50, 100];

/// The page size used when there is no stored choice, storage is unavailable,
/// or a stored value is not a member of [`OPTIONS`].
pub const DEFAULT: u32 = 20;

/// `localStorage` key for the jobs list's page size.
pub const KEY_JOBS: &str = "paavo-per-page-jobs";
/// `localStorage` key for the boards list's page size.
pub const KEY_BOARDS: &str = "paavo-per-page-boards";
/// `localStorage` key for the schedule list's page size.
pub const KEY_SCHEDULE: &str = "paavo-per-page-schedule";

/// Validate a raw stored string against [`OPTIONS`], collapsing anything
/// missing, non-numeric, or out-of-set to [`DEFAULT`]. Pure (no DOM access),
/// so it is unit-testable without a browser.
pub fn sanitize(raw: Option<String>) -> u32 {
    raw.and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|n| OPTIONS.contains(n))
        .unwrap_or(DEFAULT)
}

/// The stored page size for `key`, validated through [`sanitize`]. Returns
/// [`DEFAULT`] when the key is absent or storage is unavailable.
pub fn load(key: &str) -> u32 {
    sanitize(storage().and_then(|s| s.get_item(key).ok().flatten()))
}

/// Persist `n` as the page size for `key`. Best-effort: storage errors and
/// privacy-mode unavailability are silently ignored (matching `theme::toggle`).
pub fn store(key: &str, n: u32) {
    if let Some(s) = storage() {
        let _ = s.set_item(key, &n.to_string());
    }
}

/// `window.localStorage`, if available (absent in some privacy modes).
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn absent_is_default() {
        assert_eq!(sanitize(None), DEFAULT);
    }

    #[wasm_bindgen_test]
    fn valid_in_set_is_kept() {
        assert_eq!(sanitize(Some("30".into())), 30);
        assert_eq!(sanitize(Some("100".into())), 100);
        assert_eq!(sanitize(Some("  50 ".into())), 50);
    }

    #[wasm_bindgen_test]
    fn out_of_set_is_default() {
        assert_eq!(sanitize(Some("999".into())), DEFAULT);
        assert_eq!(sanitize(Some("25".into())), DEFAULT);
        assert_eq!(sanitize(Some("0".into())), DEFAULT);
    }

    #[wasm_bindgen_test]
    fn non_numeric_is_default() {
        assert_eq!(sanitize(Some("lots".into())), DEFAULT);
        assert_eq!(sanitize(Some(String::new())), DEFAULT);
    }
}
