//! Single-job detail page.
//!
//! Placeholder for the shell task — it already reads the `:id` route param to
//! prove parameterized routing works. Task 4.7 turns this into the live log
//! view (header + streaming frames + per-job filter).

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

/// The `/jobs/:id` detail page.
#[component]
pub fn JobDetail() -> impl IntoView {
    let params = use_params_map();
    let id = move || params.read().get("id").unwrap_or_default();
    view! {
        <h1>"Job " <span class="mono">{id}</span></h1>
        <p class="muted">"Header, live log stream, and per-job filter land here (Task 4.7)."</p>
    }
}
