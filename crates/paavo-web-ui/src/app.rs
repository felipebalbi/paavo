//! The root [`App`] component: theme bootstrap, the live-signal context, and
//! the `leptos_router` route table wrapped in the app [`Shell`].
//!
//! [`Shell`]: crate::components::shell::Shell

use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use crate::components::boards::Boards;
use crate::components::dashboard::Dashboard;
use crate::components::job_detail::JobDetail;
use crate::components::jobs_list::JobsList;
use crate::components::schedule::Schedule;
use crate::components::shell::Shell;
use crate::live::LiveSignals;
use crate::theme;

/// Application root. Mounted onto `<body>` by `main`.
#[component]
pub fn App() -> impl IntoView {
    // Bootstrap the theme before first paint (idempotent class toggle on <html>).
    theme::apply(theme::current());

    // Open the single /api/events stream and make its revision signals available
    // to every component via context, so a server-pushed bump refetches views.
    provide_context(LiveSignals::start());

    view! {
        <Router>
            <Shell>
                <Routes fallback=|| view! { <p class="muted">"Not found."</p> }>
                    <Route path=path!("/") view=Dashboard/>
                    <Route path=path!("/jobs") view=JobsList/>
                    <Route path=path!("/jobs/:id") view=JobDetail/>
                    <Route path=path!("/boards") view=Boards/>
                    <Route path=path!("/schedule") view=Schedule/>
                </Routes>
            </Shell>
        </Router>
    }
}
