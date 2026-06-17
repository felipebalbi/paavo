//! Reusable view widgets shared across pages.
//!
//! These are the small, presentation-only helpers the jobs list and job
//! detail pages both lean on: relative-time formatting against the wall
//! clock, an absolute-time tooltip companion, and the canonical job-state
//! badge. Keeping them here (rather than duplicated per page) guarantees a
//! job rendered in the list and the same job rendered on its detail page get
//! byte-identical state colours and timestamps.

use leptos::prelude::*;
use paavo_proto::{BoardHealth, JobState};
use wasm_bindgen::JsValue;

/// Format an epoch-millisecond timestamp as a terse "x ago" string relative
/// to the browser's current wall clock (`Date.now()`).
///
/// Buckets coarsen with age — `"12s ago"`, `"3m ago"`, `"4h ago"`,
/// `"2d ago"` — which is all an operator skimming a table needs; the precise
/// time is available in the `title` tooltip ([`abs_time`]). A timestamp in
/// the future (clock skew) clamps to `"0s ago"` rather than rendering a
/// negative delta.
pub fn rel_time(epoch_ms: i64) -> String {
    let now = js_sys::Date::now();
    let delta_ms = (now - epoch_ms as f64).max(0.0);
    let secs = (delta_ms / 1000.0) as u64;
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Render an epoch-millisecond timestamp as an absolute UTC string, suitable
/// for a `title=` tooltip beside a [`rel_time`] label. Uses the platform
/// `Date.toUTCString()` (e.g. `"Tue, 16 Jun 2026 18:21:45 GMT"`) so we don't
/// pull `chrono` into the wasm bundle just for a tooltip.
pub fn abs_time(epoch_ms: i64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(epoch_ms as f64));
    date.to_utc_string().as_string().unwrap_or_default()
}

/// The CSS modifier suffix for a job state, matching the `.badge.is-*`
/// classes in `style.css` (e.g. `JobState::AwaitingBoard` → `"awaiting"`,
/// `JobState::TimedOut` → `"timedout"`). Shared by [`StateBadge`] and the
/// job-detail phase banner so both key off one mapping.
pub fn state_css(state: JobState) -> &'static str {
    match state {
        JobState::Submitted => "submitted",
        JobState::Building => "building",
        JobState::Running => "running",
        JobState::AwaitingBoard => "awaiting",
        JobState::Passed => "passed",
        JobState::Failed => "failed",
        JobState::TimedOut => "timedout",
        JobState::Aborted => "aborted",
    }
}

/// Human-facing label for a job state (the badge text). Mostly the lowercase
/// variant name, with the two-word states spelled out (`"awaiting board"`,
/// `"timed out"`).
pub fn state_label(state: JobState) -> &'static str {
    match state {
        JobState::Submitted => "submitted",
        JobState::Building => "building",
        JobState::Running => "running",
        JobState::AwaitingBoard => "awaiting board",
        JobState::Passed => "passed",
        JobState::Failed => "failed",
        JobState::TimedOut => "timed out",
        JobState::Aborted => "aborted",
    }
}

/// A pill badge for a [`JobState`]: `<span class="badge is-{state}">label</span>`.
/// The `is-running` variant additionally gets a pulsing dot via CSS. Used in
/// both the jobs table and the job-detail header.
#[component]
pub fn StateBadge(
    /// The job state to render.
    state: JobState,
) -> impl IntoView {
    view! { <span class=format!("badge is-{}", state_css(state))>{state_label(state)}</span> }
}

/// The CSS modifier suffix for a board health, matching the `.badge.is-*`
/// classes in `style.css` (`Healthy` → `"healthy"`/green, `Quarantined` →
/// `"quarantined"`/red). Shared by [`HealthBadge`] and the dashboard fleet
/// list so a board's colour is identical wherever it renders.
pub fn health_css(health: BoardHealth) -> &'static str {
    match health {
        BoardHealth::Healthy => "healthy",
        BoardHealth::Quarantined => "quarantined",
    }
}

/// Human-facing label for a board health (the badge text): the lowercase
/// variant name, matching [`state_label`]'s lowercase convention so health
/// and job-state badges read the same in a table.
pub fn health_label(health: BoardHealth) -> &'static str {
    match health {
        BoardHealth::Healthy => "healthy",
        BoardHealth::Quarantined => "quarantined",
    }
}

/// A pill badge for a [`BoardHealth`]:
/// `<span class="badge is-healthy|is-quarantined">label</span>`. Used in both
/// the boards table and the dashboard fleet list.
#[component]
pub fn HealthBadge(
    /// The board health to render.
    health: BoardHealth,
) -> impl IntoView {
    view! { <span class=format!("badge is-{}", health_css(health))>{health_label(health)}</span> }
}

/// Compute the set of page numbers to surface in the pager: always the first
/// and last page, plus a ±2 window around the current page. Returned sorted
/// and de-duplicated; gaps (where consecutive numbers jump by more than one)
/// are rendered as an ellipsis by [`pager`].
pub fn page_numbers(current: u32, total_pages: u32) -> Vec<u32> {
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
/// shared `page` signal, which re-keys the caller's data resource. Shared by
/// the jobs, boards, and schedule tables so every paginated view paginates
/// identically.
pub fn pager(page: RwSignal<u32>, current: u32, total_pages: u32) -> impl IntoView {
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
