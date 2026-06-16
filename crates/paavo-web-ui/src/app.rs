//! The root [`App`] component: the `leptos_router` route table wrapped in the
//! app [`Shell`]. Live-signal context and theme bootstrap are wired in by the
//! shell/router task (4.5); for now this mounts the router and routes between
//! the placeholder pages.
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

/// Application root. Mounted onto `<body>` by `main`.
#[component]
pub fn App() -> impl IntoView {
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
