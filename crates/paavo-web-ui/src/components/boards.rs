//! Boards page (`/boards`).
//!
//! The board-fleet inventory: a paginated table of
//! [`BoardView`](paavo_proto::BoardView) rows that refreshes in place when the
//! server bumps the boards revision, plus a client-side substring filter over
//! the *loaded* page (id / kind).
//!
//! ## Why `res.get()` instead of `<Suspense>`
//!
//! Unlike the jobs list, this page layers a client-side filter on top of the
//! fetched page. Reading the `filter` signal inside a `Suspend::new(async …)`
//! body would run detached and *not* be tracked, so a keystroke wouldn't
//! re-render. Instead we read the [`LocalResource`] synchronously via
//! `res.get()` inside a plain reactive closure that also reads `filter` — the
//! same pattern the job-detail header uses — so both the live refetch and the
//! filter drive re-renders. The pagination footer keys off the *server* total
//! (the filter only narrows the current page).

use leptos::prelude::*;

use crate::api;
use crate::components::widgets::{abs_time, pager, rel_time, HealthBadge};
use crate::live::LiveSignals;

/// The `/boards` page.
#[component]
pub fn Boards() -> impl IntoView {
    let live = expect_context::<LiveSignals>();
    // 1-based page number.
    let page = RwSignal::new(1u32);
    // Client-side substring filter over the loaded page (id / kind).
    let filter = RwSignal::new(String::new());

    // One page of boards. Re-runs when the page OR the live boards revision
    // changes — the latter is how a server-pushed bump refreshes in place.
    let res = LocalResource::new(move || {
        let p = page.get();
        let _ = live.boards.get();
        async move { api::boards(p).await }
    });

    view! {
        <h1>"Boards"</h1>
        <input
            class="filter"
            r#type="text"
            autocomplete="off"
            spellcheck="false"
            placeholder="filter boards by id or kind…"
            on:input=move |ev| filter.set(event_target_value(&ev))
        />
        {move || {
            let needle = filter.get().to_lowercase();
            match res.get().map(|w| (*w).clone()) {
                None => view! { <p class="muted">"loading…"</p> }.into_any(),
                Some(Err(e)) => {
                    view! { <p class="muted">{format!("failed to load boards: {e}")}</p> }
                        .into_any()
                }
                Some(Ok(data)) => {
                    let total = data.total;
                    let cur_page = data.page;
                    let per_page = data.per_page.max(1) as u64;
                    let total_pages = total.div_ceil(per_page).max(1) as u32;
                    let rows = data
                        .items
                        .iter()
                        .filter(|b| {
                            needle.is_empty() || b.spec.id.to_lowercase().contains(&needle)
                                || b.spec.kind.to_lowercase().contains(&needle)
                        })
                        .map(|b| {
                            let id = b.spec.id.clone();
                            let kind = b.spec.kind.clone();
                            let fails = b.consecutive_infra_failures;
                            let (lu_rel, lu_abs) = match b.last_used_at {
                                Some(t) => (rel_time(t), abs_time(t)),
                                None => ("never".to_string(), String::new()),
                            };
                            let reason = b.quarantine_reason.clone().unwrap_or_default();
                            view! {
                                <tr>
                                    <td>
                                        <span class="mono">{id}</span>
                                    </td>
                                    <td>{kind}</td>
                                    <td>
                                        <HealthBadge health=b.spec.health />
                                    </td>
                                    <td>{fails}</td>
                                    <td>
                                        <span title=lu_abs>{lu_rel}</span>
                                    </td>
                                    <td class="muted">{reason}</td>
                                </tr>
                            }
                        })
                        .collect::<Vec<_>>();
                    let empty = rows.is_empty();
                    let empty_msg = if needle.is_empty() {
                        "no boards registered yet"
                    } else {
                        "no boards match the filter"
                    };
                    view! {
                        <table class="table">
                            <thead>
                                <tr>
                                    <th>"Id"</th>
                                    <th>"Kind"</th>
                                    <th>"Health"</th>
                                    <th>"Infra fails"</th>
                                    <th>"Last used"</th>
                                    <th>"Reason"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {rows}
                                {empty
                                    .then(|| {
                                        view! {
                                            <tr>
                                                <td colspan="6" class="muted">{empty_msg}</td>
                                            </tr>
                                        }
                                    })}
                            </tbody>
                        </table>
                        {(total_pages > 1).then(|| pager(page, cur_page, total_pages))}
                    }
                        .into_any()
                }
            }
        }}
    }
}
