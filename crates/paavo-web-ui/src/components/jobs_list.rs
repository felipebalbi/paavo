//! Jobs list page (`/jobs`).
//!
//! The canonical jobs index: a debounced fuzzy-search box, a paginated table
//! of [`JobListItem`](paavo_proto::JobListItem) rows, live in-place refresh
//! when the server bumps the jobs revision, and an "↑ N new" pill that lets
//! the operator pull in jobs submitted since the page was pinned.
//!
//! ## Search vs. list mode (the `as_of` pin)
//!
//! The server's `/api/jobs` has two modes (see `paavo-web`'s
//! `src/api/jobs.rs`):
//!
//! - **List mode** (blank query): rows are time-ordered and the page is
//!   *pinned* to `submitted_at <= as_of`, so paging through a busy queue sees
//!   a stable window instead of having rows shift under it. `new_count`
//!   reports how many jobs arrived after the pin — that's the "N new" pill.
//! - **Search mode** (non-blank query): fuzzy ranking over the *whole*
//!   history; the server ignores `as_of` and forces `new_count = 0`.
//!
//! So whenever the debounced query transitions we reset to page 1 and either
//! drop the pin (entering search) or re-pin to "now" (returning to list).
//!
//! ## Debounce
//!
//! Keystrokes write the raw `query` signal immediately but only commit to the
//! `dq` (debounced query) signal — which the data resource is keyed on —
//! after a 150 ms quiet period. We implement that with a monotonically
//! increasing generation counter: each keystroke schedules a
//! [`Timeout`](gloo_timers::callback::Timeout) tagged with its generation and
//! only the newest surviving timer commits. No timer handles to juggle, and
//! stale timers harmlessly no-op.

use gloo_timers::callback::Timeout;
use leptos::prelude::*;
use leptos_router::components::A;

use crate::api;
use crate::components::widgets::{abs_time, rel_time, StateBadge};
use crate::live::LiveSignals;

/// The `/jobs` list page.
#[component]
pub fn JobsList() -> impl IntoView {
    let live = expect_context::<LiveSignals>();

    // Raw input vs. debounced query. The resource keys on `dq`; `query` just
    // mirrors the live keystrokes for the (future) controlled-input cases.
    let query = RwSignal::new(String::new());
    let dq = RwSignal::new(String::new());
    // Debounce generation: bumped per keystroke; only the latest timer commits.
    let gen = RwSignal::new(0u32);
    // 1-based page number.
    let page = RwSignal::new(1u32);
    // Pin the list window to "now" at mount so pagination is stable and the
    // "N new" pill is meaningful. `None` while searching (server ignores it).
    let as_of = RwSignal::new(Some(js_sys::Date::now() as i64));

    // One page of jobs. Re-runs when the debounced query, page, pin, OR the
    // live jobs revision changes — the last of those is how a server-pushed
    // bump refreshes the table in place.
    let res = LocalResource::new(move || {
        let q = dq.get();
        let p = page.get();
        let a = as_of.get();
        let _ = live.jobs.get();
        async move { api::jobs(&q, p, a).await }
    });

    // When the committed query transitions, reset to page 1 and flip the pin:
    // dropped while searching, re-pinned to "now" when the box goes blank.
    // Skip the very first run (prev is None) so we don't refetch on mount —
    // the initial `as_of` pin set above already stands.
    Effect::new(move |prev: Option<()>| {
        let cur = dq.get();
        if prev.is_some() {
            page.set(1);
            if cur.trim().is_empty() {
                as_of.set(Some(js_sys::Date::now() as i64));
            } else {
                as_of.set(None);
            }
        }
    });

    let on_input = move |ev| {
        let value = event_target_value(&ev);
        query.set(value.clone());
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
        <h1>"Jobs"</h1>
        <input
            class="search"
            r#type="text"
            autocomplete="off"
            spellcheck="false"
            placeholder="fuzzy search jobs…"
            on:input=on_input
        />
        <Suspense fallback=move || {
            view! { <p class="muted">"loading…"</p> }
        }>
            {move || Suspend::new(async move {
                match res.await {
                    Err(e) => {
                        view! { <p class="muted">{format!("failed to load jobs: {e}")}</p> }
                            .into_any()
                    }
                    Ok(data) => {
                        let total = data.total;
                        let new_count = data.new_count;
                        let cur_page = data.page;
                        let per_page = data.per_page.max(1) as u64;
                        let total_pages = total.div_ceil(per_page).max(1) as u32;
                        let rows = data
                            .items
                            .iter()
                            .map(|it| {
                                let id = it.id.to_string();
                                let href = format!("/jobs/{id}");
                                let board = it
                                    .board_id
                                    .clone()
                                    .unwrap_or_else(|| "—".to_string());
                                let prio = format!("{:?}", it.priority);
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
                                        <td>{prio}</td>
                                        <td>{it.submitter.clone()}</td>
                                        <td>{board}</td>
                                        <td>
                                            <span title=submitted_abs>{submitted_rel}</span>
                                        </td>
                                    </tr>
                                }
                            })
                            .collect::<Vec<_>>();
                        let pill = (new_count > 0)
                            .then(|| {
                                view! {
                                    <button
                                        class="pill"
                                        on:click=move |_| {
                                            as_of.set(Some(js_sys::Date::now() as i64));
                                            page.set(1);
                                        }
                                    >
                                        {format!("↑ {new_count} new")}
                                    </button>
                                }
                            });
                        view! {
                            <div class="list-meta">
                                <span class="muted">{format!("{total} matches")}</span>
                                {pill}
                            </div>
                            <table class="table">
                                <thead>
                                    <tr>
                                        <th>"Job"</th>
                                        <th>"State"</th>
                                        <th>"Priority"</th>
                                        <th>"Submitter"</th>
                                        <th>"Board"</th>
                                        <th>"Submitted"</th>
                                    </tr>
                                </thead>
                                <tbody>{rows}</tbody>
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

/// Compute the set of page numbers to surface in the pager: always the first
/// and last page, plus a ±2 window around the current page. Returned sorted
/// and de-duplicated; gaps (where consecutive numbers jump by more than one)
/// are rendered as an ellipsis by [`pager`].
fn page_numbers(current: u32, total_pages: u32) -> Vec<u32> {
    if total_pages <= 1 {
        return vec![1];
    }
    let mut set = std::collections::BTreeSet::new();
    set.insert(1);
    set.insert(total_pages);
    let lo = current.saturating_sub(2).max(1);
    let hi = (current + 2).min(total_pages);
    for p in lo..=hi {
        set.insert(p);
    }
    set.into_iter().collect()
}

/// Render the pagination footer: a Prev button, the windowed page numbers
/// (with ellipses for gaps), and a Next button. Each control writes the
/// shared `page` signal, which re-keys the data resource.
fn pager(page: RwSignal<u32>, current: u32, total_pages: u32) -> impl IntoView {
    let mut items: Vec<AnyView> = Vec::new();
    let mut prev_n = 0u32;
    for n in page_numbers(current, total_pages) {
        if prev_n != 0 && n > prev_n + 1 {
            items.push(view! { <span class="pager-gap muted">"…"</span> }.into_any());
        }
        let is_current = n == current;
        let cls = if is_current {
            "pager-btn is-current"
        } else {
            "pager-btn"
        };
        items.push(
            view! {
                <button class=cls on:click=move |_| page.set(n) disabled=is_current>
                    {n}
                </button>
            }
            .into_any(),
        );
        prev_n = n;
    }
    let prev_disabled = current <= 1;
    let next_disabled = current >= total_pages;
    view! {
        <div class="pager">
            <button
                class="pager-btn"
                on:click=move |_| page.update(|p| *p = p.saturating_sub(1).max(1))
                disabled=prev_disabled
            >
                "‹ Prev"
            </button>
            {items}
            <button
                class="pager-btn"
                on:click=move |_| page.update(|p| { if *p < total_pages { *p += 1 } })
                disabled=next_disabled
            >
                "Next ›"
            </button>
        </div>
    }
}
