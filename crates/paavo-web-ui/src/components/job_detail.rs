//! Single-job detail page (`/jobs/:id`).
//!
//! Three stacked regions, all reactive:
//!
//! 1. **Header card** — id, [`StateBadge`], submitter/board/priority/source,
//!    and submitted/started/finished times. Sourced from `GET /api/jobs/:id`
//!    via a [`LocalResource`] keyed on the live jobs revision, so the badge
//!    and times track the job as it advances.
//! 2. **Phase banner** — the current pipeline phase, derived from the job's
//!    state but overridden in real time by `phase` SSE events.
//! 3. **Live log** — the persisted scrollback (`GET /api/jobs/:id/log`)
//!    seeded into a signal, then tailed live over the SSE proxy
//!    (`GET /api/jobs/:id/stream`). A per-job substring filter narrows the
//!    rendered lines.
//!
//! ## Streaming model
//!
//! The proxy (`paavo-web`'s `src/proxy.rs`) emits named SSE events whose
//! `data` is JSON: `frame` (a log frame enriched with `display_ts` + phase),
//! `phase`, `terminal` (closes the stream), `truncated` (closes the stream),
//! and `lagged`. We seed from the historical endpoint, open the stream with
//! `?since_seq=<last historical seq>` to skip the prefix we already have, and
//! de-dup defensively by sequence number (drop `seq <= last seen`) since the
//! client-side dedup — not the server's byte-trim — is the correctness
//! backbone.
//!
//! ## EventSource lifetime
//!
//! The `EventSource` is a `!Send` JS handle. We park it in a
//! [`StoredValue::new_local`] slot whose *handle* is `Copy + Send + Sync`
//! (the value stays thread-local), which lets the same handle be closed from
//! the terminal/truncated callbacks, from the id-change effect, and from
//! [`on_cleanup`] (whose closure must be `Send + Sync`). A generation counter
//! aborts an in-flight historical fetch whose id has since changed, so a slow
//! response can't poison a freshly-reset log. Listener closures are
//! `forget()`-ed (one bounded set per stream) per the wasm-bindgen idiom;
//! `EventSource::close()` stops them firing.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_params_map;
use paavo_proto::{LogFrame, LogLevel};
use serde::Deserialize;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{EventSource, MessageEvent};

use crate::api;
use crate::components::widgets::{abs_time, rel_time, state_css, state_label, StateBadge};
use crate::live::LiveSignals;

/// One rendered log line, normalised from either a persisted [`LogFrame`] or
/// a live `frame` SSE event so the render path is uniform.
#[derive(Clone)]
struct LogLine {
    /// Per-job sequence number (used for de-dup).
    seq: u64,
    /// Pre-formatted `mm:ss.fff` (or `H:MM:SS.fff`) since job start.
    ts: String,
    /// Severity, for the level tag + colour.
    level: LogLevel,
    /// True if this frame came from the build phase (`cargo:` target prefix).
    is_build: bool,
    /// The decoded message body (rendered as a text node — Leptos escapes it).
    message: String,
}

/// `frame` SSE payload. Extra proxy fields (`ts_us`, `phase`) are ignored.
#[derive(Deserialize)]
struct FrameEvent {
    seq: u64,
    display_ts: String,
    level: LogLevel,
    #[serde(default)]
    target: Option<String>,
    message: String,
}

impl FrameEvent {
    fn into_line(self) -> LogLine {
        let is_build = self
            .target
            .as_deref()
            .is_some_and(|t| t.starts_with("cargo:"));
        LogLine {
            seq: self.seq,
            ts: self.display_ts,
            level: self.level,
            is_build,
            message: self.message,
        }
    }
}

/// `phase` SSE payload (`{"phase":"building"|"running"}`).
#[derive(Deserialize)]
struct PhaseEvent {
    phase: String,
}

/// `terminal` SSE payload (`{"outcome": <JobOutcome JSON>}`).
#[derive(Deserialize)]
struct TerminalEvent {
    outcome: serde_json::Value,
}

/// `truncated` SSE payload (`{"reason":"..."}`).
#[derive(Deserialize)]
struct TruncatedEvent {
    reason: String,
}

/// `lagged` SSE payload (`{"missed":N}`).
#[derive(Deserialize)]
struct LaggedEvent {
    missed: u64,
}

/// Format `LogFrame::ts_us` (µs since job start) as `mm:ss.fff`, widening to
/// `H:MM:SS.fff` past an hour. Mirrors the server's `crate::time::relative_us`
/// (unpadded) so historical and live frames render identical timestamps.
fn fmt_ts_us(ts_us: u64) -> String {
    let total_ms = ts_us / 1_000;
    let ms = total_ms % 1_000;
    let total_s = total_ms / 1_000;
    let s = total_s % 60;
    let total_min = total_s / 60;
    let m = total_min % 60;
    let h = total_min / 60;
    if h == 0 {
        format!("{m:02}:{s:02}.{ms:03}")
    } else {
        format!("{h}:{m:02}:{s:02}.{ms:03}")
    }
}

/// CSS class for a log level (warn=amber, error=red; the rest muted).
fn level_css(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "lvl-trace",
        LogLevel::Debug => "lvl-debug",
        LogLevel::Info => "lvl-info",
        LogLevel::Warn => "lvl-warn",
        LogLevel::Error => "lvl-error",
    }
}

/// Upper-case label for a log level, rendered as `[INFO]` etc.
fn level_label(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "TRACE",
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    }
}

/// Normalise a persisted frame into a [`LogLine`].
fn line_from_frame(f: &LogFrame) -> LogLine {
    LogLine {
        seq: f.seq,
        ts: fmt_ts_us(f.ts_us),
        level: f.level,
        is_build: f.target.as_deref().is_some_and(|t| t.starts_with("cargo:")),
        message: f.message.clone(),
    }
}

/// Render one log line as inline spans inside the `<pre>` log pane: a
/// `[build]`/`[run]` phase tag, the relative timestamp, the `[LEVEL]`, and
/// the message, followed by a newline text node.
fn render_line(line: LogLine) -> impl IntoView {
    let (tag, tag_cls) = if line.is_build {
        ("[build]", "tag tag-build")
    } else {
        ("[run]", "tag tag-run")
    };
    let lvl_cls = format!("lvl {}", level_css(line.level));
    let lvl = format!("[{}]", level_label(line.level));
    view! {
        <span class=tag_cls>{tag}</span>
        " "
        <span class="mono ts">{line.ts}</span>
        " "
        <span class=lvl_cls>{lvl}</span>
        " "
        <span class="msg">{line.message}</span>
        "\n"
    }
}

/// Register the five named SSE listeners on `es`, wiring each into the log
/// signals. The terminal/truncated listeners also close the stream via the
/// shared `es_slot`.
fn attach_log_listeners(
    es: &EventSource,
    frames: RwSignal<Vec<LogLine>>,
    live_phase: RwSignal<Option<String>>,
    terminal_outcome: RwSignal<Option<serde_json::Value>>,
    notices: RwSignal<Vec<String>>,
    es_slot: StoredValue<Option<EventSource>, LocalStorage>,
) {
    let frame_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
        if let Some(text) = ev.data().as_string() {
            if let Ok(fe) = serde_json::from_str::<FrameEvent>(&text) {
                frames.update(|v| {
                    // De-dup by seq: append only when strictly newer than the
                    // tail (frames arrive, and seed, in increasing-seq order).
                    if v.last().is_none_or(|last| fe.seq > last.seq) {
                        v.push(fe.into_line());
                    }
                });
            }
        }
    });
    let _ = es.add_event_listener_with_callback("frame", frame_cb.as_ref().unchecked_ref());
    frame_cb.forget();

    let phase_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
        if let Some(text) = ev.data().as_string() {
            if let Ok(pe) = serde_json::from_str::<PhaseEvent>(&text) {
                live_phase.set(Some(pe.phase));
            }
        }
    });
    let _ = es.add_event_listener_with_callback("phase", phase_cb.as_ref().unchecked_ref());
    phase_cb.forget();

    let terminal_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
        if let Some(text) = ev.data().as_string() {
            if let Ok(te) = serde_json::from_str::<TerminalEvent>(&text) {
                terminal_outcome.set(Some(te.outcome));
            }
        }
        close_slot(es_slot);
    });
    let _ = es.add_event_listener_with_callback("terminal", terminal_cb.as_ref().unchecked_ref());
    terminal_cb.forget();

    let truncated_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
        if let Some(text) = ev.data().as_string() {
            if let Ok(tr) = serde_json::from_str::<TruncatedEvent>(&text) {
                notices.update(|n| n.push(format!("— stream ended: {}", tr.reason)));
            }
        }
        close_slot(es_slot);
    });
    let _ = es.add_event_listener_with_callback("truncated", truncated_cb.as_ref().unchecked_ref());
    truncated_cb.forget();

    let lagged_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
        if let Some(text) = ev.data().as_string() {
            if let Ok(lg) = serde_json::from_str::<LaggedEvent>(&text) {
                notices.update(|n| n.push(format!("— lagged: {} frame(s) dropped", lg.missed)));
            }
        }
    });
    let _ = es.add_event_listener_with_callback("lagged", lagged_cb.as_ref().unchecked_ref());
    lagged_cb.forget();
}

/// Take and close whatever `EventSource` lives in the slot (idempotent).
fn close_slot(es_slot: StoredValue<Option<EventSource>, LocalStorage>) {
    es_slot.try_update_value(|slot| {
        if let Some(es) = slot.take() {
            es.close();
        }
    });
}

/// The `/jobs/:id` detail page.
#[component]
pub fn JobDetail() -> impl IntoView {
    let live = expect_context::<LiveSignals>();
    let params = use_params_map();
    let id = move || params.read().get("id").unwrap_or_default();

    // Header: refetched on each live jobs bump so state/outcome stay current.
    let job_res = LocalResource::new(move || {
        let id = id();
        let _ = live.jobs.get();
        async move { api::job(&id).await }
    });
    // Clone out of the SendWrapper for ergonomic multi-site reads.
    let job = move || job_res.get().map(|w| (*w).clone());

    // Live log state.
    let frames = RwSignal::new(Vec::<LogLine>::new());
    let live_phase = RwSignal::new(None::<String>);
    let terminal_outcome = RwSignal::new(None::<serde_json::Value>);
    let notices = RwSignal::new(Vec::<String>::new());
    let filter = RwSignal::new(String::new());

    // EventSource handle parked in a thread-local slot (Copy/Send/Sync
    // handle) + a generation guard against stale historical fetches.
    let es_slot = StoredValue::new_local(None::<EventSource>);
    let gen = RwSignal::new(0u64);

    // (Re)establish the live log whenever the route id changes.
    Effect::new(move |_prev: Option<()>| {
        let job_id = id(); // track the param
        let my_gen = gen.get_untracked() + 1;
        gen.set(my_gen);
        // Tear down any prior stream and reset every pane.
        close_slot(es_slot);
        frames.set(Vec::new());
        live_phase.set(None);
        terminal_outcome.set(None);
        notices.set(Vec::new());

        spawn_local(async move {
            // Seed with persisted scrollback (oldest first).
            let mut since: Option<u64> = None;
            match api::job_log(&job_id, 0).await {
                Ok(hist) => {
                    if gen.get_untracked() != my_gen {
                        return; // a newer id superseded us mid-fetch
                    }
                    since = hist.last().map(|f| f.seq);
                    frames.set(hist.iter().map(line_from_frame).collect());
                }
                Err(e) => {
                    if gen.get_untracked() != my_gen {
                        return;
                    }
                    notices.update(|n| n.push(format!("could not load log history: {e}")));
                }
            }
            if gen.get_untracked() != my_gen {
                return;
            }
            // Tail the live stream, skipping the prefix we already hold.
            let url = match since {
                Some(seq) => format!("/api/jobs/{job_id}/stream?since_seq={seq}"),
                None => format!("/api/jobs/{job_id}/stream"),
            };
            if let Ok(es) = EventSource::new(&url) {
                attach_log_listeners(&es, frames, live_phase, terminal_outcome, notices, es_slot);
                es_slot.set_value(Some(es));
            }
        });
    });

    on_cleanup(move || close_slot(es_slot));

    view! {
        <h1>"Job " <span class="mono">{id}</span></h1>

        // --- header card ---
        {move || match job() {
            None => view! { <p class="muted">"loading…"</p> }.into_any(),
            Some(Err(e)) => {
                view! { <p class="muted">{format!("failed to load job: {e}")}</p> }.into_any()
            }
            Some(Ok(j)) => {
                let jid = j.id.to_string();
                let submitter = j.submitter.clone();
                let board = j.board_id.clone().unwrap_or_else(|| "—".to_string());
                let prio = format!("{:?}", j.priority);
                let source = format!("{:?}", j.source);
                let submitted = rel_time(j.submitted_at);
                let submitted_abs = abs_time(j.submitted_at);
                let started = j.started_at.map(rel_time).unwrap_or_else(|| "—".to_string());
                let finished = j.finished_at.map(rel_time).unwrap_or_else(|| "—".to_string());
                let state = j.state;
                view! {
                    <div class="card job-header">
                        <div class="job-header-top">
                            <span class="mono id-pill">{jid}</span>
                            <StateBadge state=state />
                        </div>
                        <dl class="kv">
                            <div>
                                <dt>"Submitter"</dt>
                                <dd>{submitter}</dd>
                            </div>
                            <div>
                                <dt>"Board"</dt>
                                <dd>{board}</dd>
                            </div>
                            <div>
                                <dt>"Priority"</dt>
                                <dd>{prio}</dd>
                            </div>
                            <div>
                                <dt>"Source"</dt>
                                <dd>{source}</dd>
                            </div>
                            <div>
                                <dt>"Submitted"</dt>
                                <dd title=submitted_abs>{submitted}</dd>
                            </div>
                            <div>
                                <dt>"Started"</dt>
                                <dd>{started}</dd>
                            </div>
                            <div>
                                <dt>"Finished"</dt>
                                <dd>{finished}</dd>
                            </div>
                        </dl>
                    </div>
                }
                    .into_any()
            }
        }}

        // --- phase banner ---
        {move || {
            let (css, label): (String, String) = if let Some(p) = live_phase.get() {
                match p.as_str() {
                    "building" => ("building".into(), "building".into()),
                    "running" => ("running".into(), "running".into()),
                    other => ("submitted".into(), other.to_string()),
                }
            } else {
                match job() {
                    Some(Ok(j)) => {
                        (state_css(j.state).to_string(), state_label(j.state).to_string())
                    }
                    _ => ("submitted".into(), "…".into()),
                }
            };
            view! {
                <div class="phase-banner">
                    <span class=format!("badge is-{css}")>{format!("phase: {label}")}</span>
                </div>
            }
        }}

        // --- outcome card (live terminal event wins, else the header view) ---
        {move || {
            let outcome = terminal_outcome
                .get()
                .or_else(|| match job() {
                    Some(Ok(j)) => j.outcome.as_ref().and_then(|o| serde_json::to_value(o).ok()),
                    _ => None,
                });
            outcome
                .map(|v| {
                    let pretty = serde_json::to_string_pretty(&v)
                        .unwrap_or_else(|_| v.to_string());
                    view! {
                        <div class="card outcome-card">
                            <h2>"Outcome"</h2>
                            <pre class="outcome-json">{pretty}</pre>
                        </div>
                    }
                })
        }}

        // --- per-job filter + live log ---
        <input
            class="filter"
            r#type="text"
            autocomplete="off"
            spellcheck="false"
            placeholder="filter this log…"
            on:input=move |ev| filter.set(event_target_value(&ev))
        />
        {move || {
            let needle = filter.get().to_lowercase();
            let all = frames.get();
            let total = all.len();
            let shown: Vec<LogLine> = if needle.is_empty() {
                all
            } else {
                all.into_iter()
                    .filter(|l| l.message.to_lowercase().contains(&needle))
                    .collect()
            };
            let count = shown.len();
            let lines = shown.into_iter().map(render_line).collect::<Vec<_>>();
            let note_views = notices
                .get()
                .into_iter()
                .map(|n| view! { <span class="muted">{n}</span>"\n" })
                .collect::<Vec<_>>();
            view! {
                <p class="muted">{format!("{count} / {total} lines")}</p>
                <pre class="logpane">{lines}{note_views}</pre>
            }
        }}
    }
}
