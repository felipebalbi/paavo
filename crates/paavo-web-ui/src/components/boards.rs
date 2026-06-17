//! Boards page (`/boards`).
//!
//! The board-fleet inventory: a debounced fleet filter, a paginated table of
//! [`BoardView`](paavo_proto::BoardView) rows that refreshes in place when the
//! server bumps the boards revision.
//!
//! ## Server-side filter (whole fleet, not just the page)
//!
//! The filter box narrows by an `id`/`kind` substring, but the match runs
//! **server-side over the entire `board` table** (see `paavo-web`'s
//! `src/api/boards.rs`) — *not* over the rows that happen to be on the current
//! page. So a board is found no matter which page it would otherwise land on,
//! mirroring how the jobs fuzzy search already works. The pagination footer
//! reflects the server's filtered `total`.
//!
//! ## Debounce
//!
//! Same generation-counter trick as the jobs list (`jobs_list.rs`): keystrokes
//! write the raw `filter` signal immediately but only commit to the `dq`
//! (debounced query) signal — which the data resource is keyed on — after a
//! 150 ms quiet period. Each keystroke schedules a
//! [`Timeout`](gloo_timers::callback::Timeout) tagged with its generation and
//! only the newest surviving timer commits; stale timers harmlessly no-op.
//! Committing a new query resets back to page 1.

use gloo_timers::callback::Timeout;
use leptos::prelude::*;

use crate::api;
use crate::components::widgets::{abs_time, pager, rel_time, HealthBadge};
use crate::live::LiveSignals;

/// The `/boards` page.
#[component]
pub fn Boards() -> impl IntoView {
    let live = expect_context::<LiveSignals>();

    // Raw input vs. debounced query. The resource keys on `dq`; `filter` just
    // mirrors the live keystrokes.
    let filter = RwSignal::new(String::new());
    let dq = RwSignal::new(String::new());
    // Debounce generation: bumped per keystroke; only the latest timer commits.
    let gen = RwSignal::new(0u32);
    // 1-based page number.
    let page = RwSignal::new(1u32);

    // One page of boards. Re-runs when the debounced query, page, OR the live
    // boards revision changes — the last of those is how a server-pushed bump
    // refreshes the table in place.
    let res = LocalResource::new(move || {
        let q = dq.get();
        let p = page.get();
        let _ = live.boards.get();
        async move { api::boards(p, &q).await }
    });

    // When the committed query transitions, reset to page 1. Skip the very
    // first run (prev is None) so we don't refetch on mount.
    Effect::new(move |prev: Option<()>| {
        let _ = dq.get();
        if prev.is_some() {
            page.set(1);
        }
    });

    let on_input = move |ev| {
        let value = event_target_value(&ev);
        filter.set(value.clone());
        let my_gen = gen.get_untracked() + 1;
        gen.set(my_gen);
        Timeout::new(150, move || {
            // Only commit if no newer keystroke superseded us.
            if gen.get_untracked() == my_gen {
                dq.set(value);
            }
        })
        .forget();
    };

    view! {
        <h1>"Boards"</h1>
        <input
            class="filter"
            r#type="text"
            autocomplete="off"
            spellcheck="false"
            placeholder="filter boards by id or kind…"
            on:input=on_input
        />
        <Suspense fallback=move || {
            view! { <p class="muted">"loading…"</p> }
        }>
            {move || Suspend::new(async move {
                match res.await {
                    Err(e) => {
                        view! { <p class="muted">{format!("failed to load boards: {e}")}</p> }
                            .into_any()
                    }
                    Ok(data) => {
                        let total = data.total;
                        let cur_page = data.page;
                        let per_page = data.per_page.max(1) as u64;
                        let total_pages = total.div_ceil(per_page).max(1) as u32;
                        let rows = data
                            .items
                            .iter()
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
                        // The resource is keyed on `dq`, so by the time this
                        // body re-runs `dq` already holds the query behind the
                        // current rows — read it untracked just to choose the
                        // empty-state copy.
                        let empty_msg = if dq.get_untracked().trim().is_empty() {
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
            })}
        </Suspense>
    }
}
