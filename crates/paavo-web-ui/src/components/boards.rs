//! Boards page.
//!
//! Placeholder for the shell task. Task 4.9 turns this into the paginated,
//! live board-fleet table.

use leptos::prelude::*;

/// The `/boards` page.
#[component]
pub fn Boards() -> impl IntoView {
    view! {
        <h1>"Boards"</h1>
        <p class="muted">"Paginated, live board-fleet table lands here (Task 4.9)."</p>
    }
}
