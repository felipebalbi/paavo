//! Dashboard landing page (`/`).
//!
//! The at-a-glance operator view, derived from a single consolidated
//! fetch: [`api::dashboard`] returns exact SQL aggregate counts plus two
//! short display lists (the 8 newest jobs and the relevant fleet slice).
//! The [`LocalResource`] is keyed on both the `jobs` and `boards` live
//! revisions, so a server-pushed bump on either refetches and the whole
//! dashboard recomputes in place.
//!
//! ## Accuracy of the counts
//!
//! The stat cards are exact at any scale: the counts are computed by the
//! database (`COUNT(*) ... GROUP BY state`, board health tally), not by
//! counting a capped page in the browser. "Pass rate" is all-time over
//! every retained job (bounded by the retention window). The fleet list
//! is intentionally a small, relevant slice (quarantined first, then
//! most-recently-used); the "Boards" card still reports the true
//! healthy/total for the whole fleet.

use leptos::prelude::*;
use leptos_router::components::A;
use paavo_proto::DashboardOverview;

use crate::api;
use crate::components::widgets::{abs_time, rel_time, HealthBadge, StateBadge};
use crate::live::LiveSignals;

/// The `/` landing page.
#[component]
pub fn Dashboard() -> impl IntoView {
    let live = expect_context::<LiveSignals>();

    // One consolidated fetch, refetched when either the jobs or the
    // boards revision bumps.
    let over_res = LocalResource::new(move || {
        let _ = live.jobs.get();
        let _ = live.boards.get();
        async move { api::dashboard().await }
    });

    view! {
        <h1>"Dashboard"</h1>
        {move || {
            match over_res.get().map(|w| (*w).clone()) {
                Some(Ok(o)) => render(o).into_any(),
                Some(Err(e)) => {
                    view! { <p class="muted">{format!("failed to load dashboard: {e}")}</p> }
                        .into_any()
                }
                None => view! { <p class="muted">"loading…"</p> }.into_any(),
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

/// Build the full dashboard from the consolidated overview: the stat-card
/// grid on top, then a two-column row of recent activity (wider) + the
/// board fleet (narrower) that collapses to one column on narrow screens.
fn render(over: DashboardOverview) -> impl IntoView {
    // --- stat tallies (exact, from SQL aggregates) ---
    let running = over.jobs.running;
    let queue = over.jobs.queue();
    let terminal = over.jobs.terminal();
    let pass_rate = match over.jobs.pass_rate_pct() {
        Some(p) => format!("{p}%"),
        None => "—".to_string(),
    };
    let fleet_total = over.boards.total;
    let healthy = over.boards.healthy();
    let quarantined = over.boards.quarantined;

    // --- recent activity: the newest jobs (already capped server-side) ---
    let recent = over
        .recent_jobs
        .iter()
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

    // --- board fleet: the relevant slice (quarantined first, then LRU) ---
    let fleet = over
        .fleet
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
            {stat_card(pass_rate, "Pass rate", Some(format!("{terminal} runs")))}
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
