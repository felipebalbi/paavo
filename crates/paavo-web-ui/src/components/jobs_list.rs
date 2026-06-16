//! Jobs list page.
//!
//! Placeholder for the shell task. Task 4.6 turns this into the canonical
//! component: fuzzy search, pagination, live in-place state updates, and the
//! "N new" pill.

use leptos::prelude::*;

/// The `/jobs` list page.
#[component]
pub fn JobsList() -> impl IntoView {
    view! {
        <h1>"Jobs"</h1>
        <p class="muted">"Fuzzy search + paginated, live jobs table lands here (Task 4.6)."</p>
    }
}
