//! HTTP client + SSE proxy that bridges paavod's NDJSON
//! `/jobs/:id/stream` body into browser-friendly Server-Sent Events
//! at paavo-web's `/api/jobs/:id/stream`.
//!
//! ## Why proxy?
//!
//! paavod publishes one [`paavo_proto::WireMessage`] per NDJSON line
//! over an `application/x-ndjson` body. That's perfectly readable by
//! `paavo-cli`, but `EventSource` in the browser only understands
//! the SSE wire format. paavo-web could instruct the browser to
//! `fetch()` paavod directly and parse NDJSON in JS, but that
//! requires CORS headers on paavod and exposes the daemon's port to
//! every viewer's browser. Cleaner: viewers talk only to paavo-web,
//! paavo-web talks to paavod, paavod stays a backend service.
//!
//! ## Wire transformation
//!
//! Each upstream NDJSON line is deserialised as `WireMessage`, then
//! re-emitted as a named SSE event with the same JSON payload (plus
//! a couple of proxy-side enrichments — see `display_ts` and
//! `phase` below). Variant → event-name mapping:
//!
//! | WireMessage variant | SSE event name | data payload                                                 |
//! |---------------------|----------------|--------------------------------------------------------------|
//! | `Frame { frame }`   | `frame`        | LogFrame fields + `display_ts: "mm:ss.fff"` + current phase  |
//! | `Phase { phase }`   | `phase`        | `{"phase": "building" \| "running"}`                          |
//! | `Lagged { missed }` | `lagged`       | `{"missed": N}`                                              |
//! | `Terminal { o }`    | `terminal`     | `{"outcome": <JobOutcome JSON>}` — closes the stream         |
//! | `Truncated { r }`   | `truncated`    | `{"reason": "..."}` — closes the stream                      |
//!
//! ## Phase enrichment
//!
//! The proxy keeps a `current_phase` cursor across the deserialised
//! upstream stream. When a `Phase` message arrives the cursor
//! updates; when a `Frame` arrives the proxy *adds* a `phase` field
//! to the JSON payload it emits. This means a JS consumer doesn't
//! have to track its own state machine — every `frame` event is
//! self-describing. `Phase` events are still emitted (so the JS can
//! keep a banner in sync), but frame phase-tagging is entirely on
//! the proxy.
//!
//! ## Failure modes
//!
//! - 400 (plain text) on a non-ULID job id.
//! - 404 / 500 / etc. (plain text) on paavod's own status code,
//!   passed through.
//! - 502 (plain text) on paavod connect error (paavod down /
//!   misconfigured `paavod_url`).
//! - 200 SSE stream that emits exactly one `truncated` event and
//!   closes for every other class of upstream failure (parse error
//!   mid-stream, byte stream error, EOF without terminal).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use bytes::{Bytes, BytesMut};
use futures::stream::StreamExt;
use paavo_proto::{JobId, JobPhase, WireMessage};
use serde_json::json;
use std::convert::Infallible;
use std::str::FromStr;
use std::time::Duration;

/// Cloneable HTTP client + base URL pair, used by paavo-web's
/// `/api/jobs/:id/stream` to reach paavod's NDJSON endpoint.
///
/// Constructed once at startup, lives on `AppState`. The underlying
/// `reqwest::Client` already shares connection pools across clones,
/// so cloning the wrapper is cheap.
#[derive(Clone)]
pub struct PaavodClient {
    /// reqwest client. Configured with a 5 s connect timeout (so a
    /// downed paavod surfaces as 502 quickly) but NO request
    /// timeout — streaming bodies need to live for the full duration
    /// of a job, which can be tens of minutes for soak runs.
    pub http: reqwest::Client,
    /// `paavod_url` from paavo.toml. Resolved to a `reqwest::Url` at
    /// construction so URL parsing errors fail at startup rather
    /// than per request.
    pub base_url: reqwest::Url,
}

impl PaavodClient {
    /// Build a client around the paavod base URL from paavo.toml.
    /// Fails on a malformed URL — at startup, so paavo-web doesn't
    /// silently start up against a broken config.
    pub fn new(base_url: &str) -> anyhow::Result<Self> {
        let base_url = reqwest::Url::parse(base_url)
            .map_err(|e| anyhow::anyhow!("invalid paavod_url {base_url:?}: {e}"))?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(90))
            .user_agent(concat!("paavo-web/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| anyhow::anyhow!("build reqwest client: {e}"))?;
        Ok(Self { http, base_url })
    }
}

/// Combined router state: the read-only sqlite handle the pages
/// already use, plus the upstream paavod HTTP client used by the SSE
/// proxy. Each page handler can keep its existing
/// `State<WebDb>` extractor thanks to `FromRef` (declared in
/// `app.rs`).
#[derive(Clone)]
pub struct AppState {
    /// Read-only sqlite handle.
    pub db: crate::db::WebDb,
    /// paavod HTTP client (for the SSE proxy; pages don't use it).
    pub paavod: PaavodClient,
}

/// `GET /api/jobs/:id/stream` — open a streaming connection to
/// paavod's NDJSON endpoint, deserialise each line as
/// [`WireMessage`], re-emit as a named SSE event. Closes after a
/// terminal/truncated event.
pub async fn stream_job(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    // 1. Validate the job id format. paavod also validates, but
    //    bouncing here saves an upstream round trip and keeps the
    //    error wording consistent with what paavo-cli emits for the
    //    same condition.
    let _id: JobId = match JobId::from_str(&id) {
        Ok(i) => i,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid job id").into_response(),
    };

    // 2. Open the upstream stream.
    let upstream_url = match s.paavod.base_url.join(&format!("/jobs/{id}/stream")) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(error = %e, base = %s.paavod.base_url, "join /jobs/:id/stream onto base_url");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("paavod_url join: {e}"),
            )
                .into_response();
        }
    };
    let resp = match s.paavod.http.get(upstream_url.clone()).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, %upstream_url, "paavod unreachable");
            return (
                StatusCode::BAD_GATEWAY,
                format!("paavod unreachable: {e}"),
            )
                .into_response();
        }
    };
    if !resp.status().is_success() {
        // Pass paavod's status + body straight back. `paavo-cli` and
        // paavo-web's other pages don't see this — only the SSE
        // proxy — so 404/400/500 from paavod surface verbatim.
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return (status, body).into_response();
    }

    // 3. Wrap the upstream byte stream into an SSE-event stream.
    let upstream_bytes = resp.bytes_stream();
    let sse_stream = ndjson_to_sse(upstream_bytes);

    // 4. KeepAlive comments every 15 s so corporate reverse proxies
    //    (and the browser's idle-tab timer) don't kill an idle
    //    connection during a long compile. SSE comments (`:keep-
    //    alive\n\n`) are explicitly ignored by `EventSource`.
    Sse::new(sse_stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

/// Adapter: take an upstream chunked byte stream from reqwest, split
/// on `\n`, deserialise each line as [`WireMessage`], yield one named
/// SSE [`Event`] per line. Closes the stream after a `Terminal` or
/// `Truncated` event (or on upstream EOF / parse error / IO error,
/// after emitting a synthetic `truncated`).
///
/// Generic over the byte-stream type so tests can wrap an in-memory
/// `Vec<Bytes>` without spinning up reqwest.
fn ndjson_to_sse<S, E>(s: S) -> impl futures::Stream<Item = Result<Event, Infallible>>
where
    S: futures::Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    async_stream::stream! {
        let mut buf = BytesMut::new();
        let mut current_phase: Option<JobPhase> = None;
        // Pin the stream so we can `next().await` it inside the
        // async-stream macro. Using `tokio::pin!` keeps the original
        // `s` on the stack rather than boxing it.
        let mut s = std::pin::pin!(s);

        loop {
            match s.next().await {
                Some(Ok(chunk)) => {
                    buf.extend_from_slice(&chunk);
                    while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
                        let line_bytes = buf.split_to(nl + 1);
                        // strip trailing '\n'
                        let line = &line_bytes[..line_bytes.len() - 1];
                        if line.is_empty() {
                            continue;
                        }
                        match serde_json::from_slice::<WireMessage>(line) {
                            Ok(WireMessage::Phase { phase }) => {
                                current_phase = Some(phase);
                                yield Ok(named_event(
                                    "phase",
                                    json!({"phase": phase}),
                                ));
                            }
                            Ok(WireMessage::Frame { frame }) => {
                                // Enrichment: server-side format ts_us
                                // → "mm:ss.fff" so the JS doesn't need
                                // a duplicate formatter (the same
                                // crate::time helper renders historical
                                // frames during SSR). And tag with the
                                // current phase so the JS dispatcher
                                // is stateless.
                                let display_ts = crate::time::relative_us(frame.ts_us, false);
                                let payload = json!({
                                    "seq": frame.seq,
                                    "ts_us": frame.ts_us,
                                    "display_ts": display_ts,
                                    "level": frame.level,
                                    "target": frame.target,
                                    "message": frame.message,
                                    "phase": current_phase,
                                });
                                yield Ok(named_event("frame", payload));
                            }
                            Ok(WireMessage::Lagged { missed }) => {
                                yield Ok(named_event("lagged", json!({"missed": missed})));
                            }
                            Ok(WireMessage::Terminal { outcome }) => {
                                yield Ok(named_event("terminal", json!({"outcome": outcome})));
                                return;
                            }
                            Ok(WireMessage::Truncated { reason }) => {
                                yield Ok(named_event("truncated", json!({"reason": reason})));
                                return;
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, line = %String::from_utf8_lossy(line), "malformed NDJSON line from paavod");
                                yield Ok(named_event(
                                    "truncated",
                                    json!({"reason": format!("malformed upstream frame: {e}")}),
                                ));
                                return;
                            }
                        }
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "upstream stream error");
                    yield Ok(named_event(
                        "truncated",
                        json!({"reason": format!("upstream stream error: {e}")}),
                    ));
                    return;
                }
                None => {
                    // Upstream closed without a Terminal/Truncated
                    // event. Synthesise one so the client can update
                    // its UI to "stream closed" instead of waiting
                    // indefinitely.
                    yield Ok(named_event(
                        "truncated",
                        json!({"reason": "upstream closed without terminal"}),
                    ));
                    return;
                }
            }
        }
    }
}

/// Build an SSE [`Event`] with a named event type and a JSON data
/// payload. Helper exists so the call sites above stay one-liners
/// and don't repeat the `.event(...).json_data(...).unwrap()` chain.
///
/// `json_data` only fails for non-serialisable values (recursion,
/// non-UTF-8 strings); none of our payloads can hit either, so the
/// `unwrap` is a hard "this is impossible" assertion. If a future
/// payload field changes that, the panic surfaces at the failing
/// SSE-emit instead of being hidden in a Result.
fn named_event(event: &str, payload: serde_json::Value) -> Event {
    Event::default()
        .event(event)
        .json_data(payload)
        .expect("SSE payload always serialises (no recursion, no non-UTF-8)")
}
