//! Live revision signals fed by the consolidated `/api/events` SSE channel.
//!
//! paavo-web collapses every live update into one `EventSource`: on connect it
//! pushes a `snapshot` event carrying all three resource revisions at once,
//! then one named event per *changed* resource (`jobs` / `boards` /
//! `schedules`) whose `data` is the new revision number (see paavo-web's
//! `src/api/events.rs`). We open that single stream and translate each event
//! into a `set()` on the matching [`RwSignal<u64>`].
//!
//! Components subscribe by *reading* the relevant revision signal inside a
//! resource source (e.g. `LocalResource::new(move || { live.jobs.get(); ... })`)
//! so a server-pushed bump transparently re-runs the fetch and refreshes the
//! current view — no client-side merge/dedup logic.

use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::JsCast;
use web_sys::{EventSource, MessageEvent};

/// The three per-resource revision counters. Each [`RwSignal`] is `Copy`, so
/// this whole struct is `Copy` and is shared app-wide via `provide_context`.
#[derive(Clone, Copy)]
pub struct LiveSignals {
    /// Bumped when the jobs index changes.
    pub jobs: RwSignal<u64>,
    /// Bumped when the board fleet changes.
    pub boards: RwSignal<u64>,
    /// Bumped when the schedule set changes.
    pub schedules: RwSignal<u64>,
}

/// JSON payload of the `snapshot` event — mirrors paavo-web's `Revisions`
/// (`{ "jobs": N, "boards": N, "schedules": N }`).
#[derive(Deserialize)]
struct Snapshot {
    jobs: u64,
    boards: u64,
    schedules: u64,
}

impl LiveSignals {
    /// Create the three signals, open the `/api/events` `EventSource`, and
    /// register the listeners that drive them. Call once at the app root
    /// (inside a reactive owner) and `provide_context` the result.
    ///
    /// If the browser refuses to construct the `EventSource` (should not happen
    /// for a same-origin URL), the signals simply stay at their initial values
    /// and the UI falls back to its one-shot initial fetch — a benign
    /// degradation rather than a panic.
    pub fn start() -> Self {
        let signals = Self {
            jobs: RwSignal::new(0),
            boards: RwSignal::new(0),
            schedules: RwSignal::new(0),
        };
        if let Ok(source) = EventSource::new("/api/events") {
            signals.attach(&source);
        }
        signals
    }

    /// Register the `snapshot` + per-resource listeners on an open stream.
    ///
    /// The listener closures must outlive this function (the `EventSource`
    /// keeps firing for the app's lifetime), so each is `forget()`-ed — the
    /// standard wasm-bindgen idiom for an app-lifetime singleton. Leaking a
    /// handful of closures once is intentional and bounded.
    fn attach(&self, source: &EventSource) {
        // Per-resource events: `data` is the new revision as a decimal string.
        for (name, signal) in [
            ("jobs", self.jobs),
            ("boards", self.boards),
            ("schedules", self.schedules),
        ] {
            let cb = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
                if let Some(text) = ev.data().as_string() {
                    if let Ok(rev) = text.trim().parse::<u64>() {
                        signal.set(rev);
                    }
                }
            });
            let _ = source.add_event_listener_with_callback(name, cb.as_ref().unchecked_ref());
            cb.forget();
        }

        // Snapshot event: JSON setting all three revisions at once (sent on
        // connect, so a fresh or reconnected client re-syncs with no replay).
        let Self {
            jobs,
            boards,
            schedules,
        } = *self;
        let snap = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
            if let Some(text) = ev.data().as_string() {
                if let Ok(s) = serde_json::from_str::<Snapshot>(&text) {
                    jobs.set(s.jobs);
                    boards.set(s.boards);
                    schedules.set(s.schedules);
                }
            }
        });
        let _ = source.add_event_listener_with_callback("snapshot", snap.as_ref().unchecked_ref());
        snap.forget();
    }
}
