//! Dashboard landing page (`/`).
//!
//! The at-a-glance operator view, derived entirely from two fetched windows:
//! a recent jobs page and the board fleet. Both are [`LocalResource`]s keyed
//! on their matching live revision, so a server-pushed `jobs` / `boards` bump
//! refetches and the whole dashboard recomputes in place.
//!
//! ## Accuracy of the counts
//!
//! In-flight jobs (`Running` / `Building` / `AwaitingBoard` / `Submitted`) are
//! always the *newest* rows, so counting them within a recent window
//! (`per_page=200`, the server's clamp ceiling) is exact, not an estimate.
//! The "pass rate (recent)" is explicitly scoped to that window — it is the
//! recent rate, not an all-time figure. The fleet stats fetch up to 100 boards
//! (the server's ceiling), which covers any realistic lab in one request.

use leptos::prelude::*;
use leptos_router::components::A;
use paavo_proto::{BoardHealth, BoardView, JobListItem, JobState, Page};

use crate::api;
use crate::components::widgets::{abs_time, rel_time, HealthBadge, StateBadge};
use crate::live::LiveSignals;

/// The `/` landing page.
#[component]
pub fn Dashboard() -> impl IntoView {
    let live = expect_context::<LiveSignals>();

    // A wide, newest-first jobs window — large enough that every in-flight job
    // is captured (they are always the newest rows). Refetched on a jobs bump.
    let jobs_res = LocalResource::new(move || {
        let _ = live.jobs.get();
        async move { api::jobs_page("", 1, 200, None).await }
    });
    // The board fleet (up to the server's 100-row ceiling). Refetched on a
    // boards bump.
    let boards_res = LocalResource::new(move || {
        let _ = live.boards.get();
        async move { api::boards_page(1, 100).await }
    });

    view! {
        <h1>"Dashboard"</h1>
        {move || {
            let jobs = jobs_res.get().map(|w| (*w).clone());
            let boards = boards_res.get().map(|w| (*w).clone());
            match (jobs, boards) {
                (Some(Ok(j)), Some(Ok(b))) => render(j, b).into_any(),
                (Some(Err(e)), _) | (_, Some(Err(e))) => {
                    view! { <p class="muted">{format!("failed to load dashboard: {e}")}</p> }
                        .into_any()
                }
                _ => view! { <p class="muted">"loading…"</p> }.into_any(),
            }
        }}
    }
}

/// One stat card: a big value, its label, and an optional muted subtitle.
fn stat_card(value: String, label: &'static str, sub: Option<String>) -> impl IntoView {
    view! {
        <div class="stat card">
            <span class="stat-value">{value}</span>
            <span class="stat-label">{label}</span>
            {sub.map(|s| view! { <span class="stat-sub">{s}</span> })}
        </div>
    }
}

/// Build the full dashboard from the two fetched windows: the stat-card grid
/// on top, then a two-column row of recent activity (wider) + the board fleet
/// (narrower) that collapses to one column on narrow screens.
fn render(jobs: Page<JobListItem>, boards: Page<BoardView>) -> impl IntoView {
    // --- stat tallies over the jobs window ---
    let running = jobs
        .items
        .iter()
        .filter(|i| i.state == JobState::Running)
        .count();
    let queue = jobs
        .items
        .iter()
        .filter(|i| {
            matches!(
                i.state,
                JobState::Submitted | JobState::Building | JobState::AwaitingBoard
            )
        })
        .count();
    let mut terminal = 0usize;
    let mut passed = 0usize;
    for i in &jobs.items {
        if i.state.is_terminal() {
            terminal += 1;
            if i.state == JobState::Passed {
                passed += 1;
            }
        }
    }
    let pass_rate = if terminal > 0 {
        format!(
            "{}%",
            (passed as f64 / terminal as f64 * 100.0).round() as u64
        )
    } else {
        "—".to_string()
    };

    // --- fleet tallies over the boards window ---
    let fleet_total = boards.items.len();
    let healthy = boards
        .items
        .iter()
        .filter(|b| b.spec.health == BoardHealth::Healthy)
        .count();
    let quarantined = fleet_total - healthy;

    // --- recent activity: the 8 newest jobs ---
    let recent = jobs
        .items
        .iter()
        .take(8)
        .map(|it| {
            let id = it.id.to_string();
            let href = format!("/jobs/{id}");
            let submitted_rel = rel_time(it.submitted_at);
            let submitted_abs = abs_time(it.submitted_at);
            view! {
                <tr>
                    <td>
                        <A href=href>
                            <span class="mono">{id}</span>
                        </A>
                    </td>
                    <td>
                        <StateBadge state=it.state />
                    </td>
                    <td>{it.submitter.clone()}</td>
                    <td>
                        <span title=submitted_abs>{submitted_rel}</span>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>();
    let no_recent = recent.is_empty();

    // --- board fleet: a compact health + last-used list ---
    let fleet = boards
        .items
        .iter()
        .map(|b| {
            let id = b.spec.id.clone();
            let (lu_rel, lu_abs) = match b.last_used_at {
                Some(t) => (rel_time(t), abs_time(t)),
                None => ("never".to_string(), String::new()),
            };
            view! {
                <tr>
                    <td>
                        <span class="mono">{id}</span>
                    </td>
                    <td>
                        <HealthBadge health=b.spec.health />
                    </td>
                    <td>
                        <span title=lu_abs>{lu_rel}</span>
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>();
    let no_fleet = fleet.is_empty();

    view! {
        <div class="stats">
            {stat_card(running.to_string(), "Running", None)}
            {stat_card(queue.to_string(), "Queue", None)}
            {stat_card(
                format!("{healthy}/{fleet_total}"),
                "Boards",
                (quarantined > 0).then(|| format!("{quarantined} quarantined")),
            )}
            {stat_card(pass_rate, "Pass rate (recent)", Some(format!("{terminal} runs")))}
        </div>
        <div class="grid2">
            <div class="card">
                <h2 class="card-title">"Recent activity"</h2>
                <table class="table">
                    <tbody>
                        {recent}
                        {no_recent
                            .then(|| {
                                view! {
                                    <tr>
                                        <td colspan="4" class="muted">"no jobs yet"</td>
                                    </tr>
                                }
                            })}
                    </tbody>
                </table>
            </div>
            <div class="card">
                <h2 class="card-title">"Board fleet"</h2>
                <table class="table">
                    <tbody>
                        {fleet}
                        {no_fleet
                            .then(|| {
                                view! {
                                    <tr>
                                        <td colspan="3" class="muted">"no boards registered"</td>
                                    </tr>
                                }
                            })}
                    </tbody>
                </table>
            </div>
        </div>
    }
}
