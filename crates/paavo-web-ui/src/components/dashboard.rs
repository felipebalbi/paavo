//! Dashboard landing page.
//!
//! Placeholder for the shell task. Task 4.8 turns this into the live dashboard
//! (stat cards, recent activity, board-fleet health).

use leptos::prelude::*;

/// The `/` landing page.
#[component]
pub fn Dashboard() -> impl IntoView {
    view! {
        <h1>"Dashboard"</h1>
        <p class="muted">"Live stat cards, recent activity, and fleet health land here (Task 4.8)."</p>
    }
}
