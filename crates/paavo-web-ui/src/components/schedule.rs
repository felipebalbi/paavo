//! Schedule page (`/schedule`).
//!
//! The cron-schedule registry: a paginated table of
//! [`ScheduleView`](paavo_proto::ScheduleView) rows that refreshes in place
//! when the server bumps the schedules revision. No client-side filter, but
//! like the jobs and boards pages the table is wrapped in `<Transition>` so a
//! live refetch keeps the current rows on screen instead of flashing the
//! loading fallback.

use leptos::prelude::*;

use crate::api;
use crate::components::widgets::{abs_time, pager, per_page_selector, rel_time};
use crate::live::LiveSignals;
use crate::per_page;

/// The `/schedule` page.
#[component]
pub fn Schedule() -> impl IntoView {
    let live = expect_context::<LiveSignals>();
    // 1-based page number.
    let page = RwSignal::new(1u32);
    // Rows-per-page, restored from this list's browser-local preference.
    let per_page = RwSignal::new(per_page::load(per_page::KEY_SCHEDULE));

    // One page of schedules. Re-runs when the page OR the live schedules
    // revision changes — the latter is how a server-pushed bump refreshes.
    let res = LocalResource::new(move || {
        let p = page.get();
        let pp = per_page.get();
        let _ = live.schedules.get();
        async move { api::schedules_page(p, pp).await }
    });

    // Persist the page-size choice and jump back to page 1 whenever it changes.
    // First run (prev = None) is the mount; restoring a stored size must NOT
    // reset paging.
    Effect::new(move |prev: Option<u32>| {
        let pp = per_page.get();
        if let Some(old) = prev {
            if old != pp {
                per_page::store(per_page::KEY_SCHEDULE, pp);
                page.set(1);
            }
        }
        pp
    });

    view! {
        <h1>"Schedule"</h1>
        <Transition fallback=move || {
            view! { <p class="muted">"loading…"</p> }
        }>
            {move || Suspend::new(async move {
                match res.await {
                    Err(e) => {
                        view! { <p class="muted">{format!("failed to load schedules: {e}")}</p> }
                            .into_any()
                    }
                    Ok(data) => {
                        let total = data.total;
                        let cur_page = data.page;
                        let cur_per_page = data.per_page;
                        let per_page_n = data.per_page.max(1) as u64;
                        let total_pages = total.div_ceil(per_page_n).max(1) as u32;
                        let empty = data.items.is_empty();
                        let rows = data
                            .items
                            .iter()
                            .map(|s| {
                                let id = s.id.clone();
                                let cron = s.cron.clone();
                                let (en_css, en_label) = if s.enabled {
                                    ("enabled", "enabled")
                                } else {
                                    ("disabled", "disabled")
                                };
                                let (trig_rel, trig_abs) = match s.last_triggered_at {
                                    Some(t) => (rel_time(t), abs_time(t)),
                                    None => ("never".to_string(), String::new()),
                                };
                                let (done_rel, done_abs) = match s.last_completed_at {
                                    Some(t) => (rel_time(t), abs_time(t)),
                                    None => ("never".to_string(), String::new()),
                                };
                                view! {
                                    <tr>
                                        <td>
                                            <span class="mono">{id}</span>
                                        </td>
                                        <td>
                                            <code class="mono">{cron}</code>
                                        </td>
                                        <td>
                                            <span class=format!(
                                                "badge is-{en_css}",
                                            )>{en_label}</span>
                                        </td>
                                        <td>
                                            <span title=trig_abs>{trig_rel}</span>
                                        </td>
                                        <td>
                                            <span title=done_abs>{done_rel}</span>
                                        </td>
                                    </tr>
                                }
                            })
                            .collect::<Vec<_>>();
                        view! {
                            <table class="table">
                                <thead>
                                    <tr>
                                        <th>"Id"</th>
                                        <th>"Cron"</th>
                                        <th>"Enabled"</th>
                                        <th>"Last triggered"</th>
                                        <th>"Last completed"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {rows}
                                    {empty
                                        .then(|| {
                                            view! {
                                                <tr>
                                                    <td colspan="5" class="muted">
                                                        "no schedules registered yet"
                                                    </td>
                                                </tr>
                                            }
                                        })}
                                </tbody>
                            </table>
                            <div class="list-footer">
                                {per_page_selector(per_page, cur_per_page)}
                                {(total_pages > 1)
                                    .then(|| pager(page, cur_page, total_pages))}
                            </div>
                        }
                            .into_any()
                    }
                }
            })}
        </Transition>
    }
}
