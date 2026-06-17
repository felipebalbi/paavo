//! End-to-end test of paavo-web's SSE proxy against an in-process
//! fake paavod. Verifies that NDJSON `WireMessage` lines from
//! "paavod" surface as named SSE events with the right payload,
//! including the proxy-side phase enrichment and the synthetic
//! `truncated` event when upstream closes without a terminal.

use axum::body::{to_bytes, Body};
use axum::http::Request;
use paavo_db::Db;
use paavo_proto::{JobOutcome, JobPhase, LogFrame, LogLevel, WireMessage};
use paavo_web::db::WebDb;
use paavo_web::proxy::{AppState, PaavodClient};
use std::net::SocketAddr;
use tempfile::tempdir;
use tower::ServiceExt;

/// Spawn an in-process axum server that responds to
/// `GET /jobs/:id/stream` with the supplied NDJSON body. Returns the
/// bound `SocketAddr` so paavo-web's `PaavodClient` can be pointed at
/// it. The server lives until the returned `tokio::task::JoinHandle`
/// is dropped at the end of the test (held implicitly via the `_g`
/// guard in each test below — not joined, just dropped to abort).
async fn spawn_fake_paavod(ndjson_body: &'static str) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    use axum::extract::Path;
    use axum::http::header;
    use axum::response::Response;
    use axum::routing::get;
    use axum::Router;

    async fn stream_handler(
        Path(_id): Path<String>,
        axum::Extension(body): axum::Extension<&'static str>,
    ) -> Response {
        Response::builder()
            .status(200)
            .header(header::CONTENT_TYPE, "application/x-ndjson")
            .body(Body::from(body))
            .unwrap()
    }

    let app = Router::new()
        .route("/jobs/:id/stream", get(stream_handler))
        .layer(axum::Extension(ndjson_body));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, handle)
}

/// Build paavo-web's router pointing at the fake paavod's URL.
fn paavo_web_router(paavod_addr: SocketAddr) -> (tempfile::TempDir, axum::Router) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("paavo.sqlite");
    let _ = Db::open(&path).unwrap(); // run migrations
    let db = WebDb::open(&path).unwrap();
    let paavod = PaavodClient::new(&format!("http://{paavod_addr}")).expect("valid URL");
    let state = AppState {
        db,
        paavod,
        live: paavo_web::index::LiveState::new(),
    };
    let app = paavo_web::app::build_router(state);
    (dir, app)
}

/// Drain the SSE response body and return the raw bytes as UTF-8.
/// SSE bodies are line-oriented `event:`/`data:`/blank-line records;
/// each test below string-matches against this verbatim.
async fn fetch_sse_body(app: axum::Router, uri: &str) -> String {
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 from SSE route");
    // The KeepAlive layer makes the body unbounded in the general
    // case, so the test fixtures are short and we cap at 1 MiB.
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Build one NDJSON line for the given message + a trailing `\n`,
/// using the SAME serialiser paavod uses on its producer side.
fn ndjson_line(msg: &WireMessage) -> String {
    let mut s = serde_json::to_string(msg).unwrap();
    s.push('\n');
    s
}

#[tokio::test]
async fn proxy_emits_named_sse_events_for_each_wire_variant() {
    // Mint a deterministic NDJSON body that exercises every
    // WireMessage variant in order: Phase(Building), Frame, Phase
    // (Running), Frame, Terminal. The proxy's `current_phase`
    // cursor should tag the two Frame events with the matching
    // phase. Terminal closes the stream.
    let mut body = String::new();
    body.push_str(&ndjson_line(&WireMessage::Phase {
        phase: JobPhase::Building,
    }));
    body.push_str(&ndjson_line(&WireMessage::Frame {
        frame: LogFrame {
            seq: 0,
            ts_us: 12_345_000, // 12.345 s — should render as "00:12.345"
            level: LogLevel::Info,
            target: Some("cargo:stderr".into()),
            message: "Compiling foo v0.1.0".into(),
        },
    }));
    body.push_str(&ndjson_line(&WireMessage::Phase {
        phase: JobPhase::Running,
    }));
    body.push_str(&ndjson_line(&WireMessage::Frame {
        frame: LogFrame {
            seq: 1,
            ts_us: 45_000_000, // 45.000 s
            level: LogLevel::Info,
            target: Some("app::dma".into()),
            message: "Test OK".into(),
        },
    }));
    body.push_str(&ndjson_line(&WireMessage::Terminal {
        outcome: JobOutcome::Passed,
    }));
    let body: &'static str = Box::leak(body.into_boxed_str());

    let (paavod_addr, _g) = spawn_fake_paavod(body).await;
    let (_d, app) = paavo_web_router(paavod_addr);
    let sse = fetch_sse_body(app, "/api/jobs/01ARZ3NDEKTSV4RRFFQ69G5FAV/stream").await;

    // Five named events in order. SSE wire format: each event is
    // `event: <name>\ndata: <one-line JSON>\n\n` — we string-match
    // against substrings rather than parsing the full SSE record set
    // (the contract here is "the named events are present", not
    // "the proxy uses N spaces between : and value").
    assert!(
        sse.contains("event: phase\n"),
        "missing phase events; body:\n{sse}"
    );
    assert!(
        sse.contains("event: frame\n"),
        "missing frame events; body:\n{sse}"
    );
    assert!(
        sse.contains("event: terminal\n"),
        "missing terminal event; body:\n{sse}"
    );

    // Order: the first phase appears BEFORE any frame, the second
    // phase BETWEEN the two frames. Walk indices.
    let first_phase = sse.find("event: phase").expect("phase event");
    let first_frame = sse.find("event: frame").expect("frame event");
    let terminal = sse.find("event: terminal").expect("terminal event");
    assert!(
        first_phase < first_frame,
        "phase did not precede first frame"
    );
    assert!(first_frame < terminal, "frame did not precede terminal");

    // Frame phase enrichment: the first frame carries
    // `"phase":"building"`, the second `"phase":"running"`. These are
    // the proxy's enrichments — they're NOT in upstream's wire bytes.
    assert!(
        sse.contains(r#""phase":"building""#),
        "first frame missing building-phase tag; body:\n{sse}"
    );
    assert!(
        sse.contains(r#""phase":"running""#),
        "second frame missing running-phase tag; body:\n{sse}"
    );

    // display_ts enrichment: server-side formatted timestamp.
    assert!(
        sse.contains(r#""display_ts":"00:12.345""#),
        "first frame missing display_ts; body:\n{sse}"
    );
    assert!(
        sse.contains(r#""display_ts":"00:45.000""#),
        "second frame missing display_ts; body:\n{sse}"
    );

    // Terminal payload contains the JobOutcome.
    assert!(
        sse.contains(r#""outcome":"passed""#),
        "terminal missing passed outcome; body:\n{sse}"
    );
}

#[tokio::test]
async fn proxy_synthesises_truncated_when_upstream_closes_without_terminal() {
    // Upstream emits one Frame and then closes the body with no
    // Terminal/Truncated event. The proxy MUST synthesise a
    // `truncated` SSE event so the client UI can flip to "stream
    // closed" instead of waiting on a connection that's already
    // gone.
    let mut body = String::new();
    body.push_str(&ndjson_line(&WireMessage::Frame {
        frame: LogFrame {
            seq: 0,
            ts_us: 0,
            level: LogLevel::Info,
            target: None,
            message: "abandoned".into(),
        },
    }));
    let body: &'static str = Box::leak(body.into_boxed_str());

    let (paavod_addr, _g) = spawn_fake_paavod(body).await;
    let (_d, app) = paavo_web_router(paavod_addr);
    let sse = fetch_sse_body(app, "/api/jobs/01ARZ3NDEKTSV4RRFFQ69G5FAV/stream").await;

    assert!(
        sse.contains("event: frame\n"),
        "lost the legitimate frame; body:\n{sse}"
    );
    assert!(
        sse.contains("event: truncated\n"),
        "missing synthetic truncated event; body:\n{sse}"
    );
    assert!(
        sse.contains(r#""reason":"upstream closed without terminal""#),
        "wrong truncated reason; body:\n{sse}"
    );
}

#[tokio::test]
async fn proxy_synthesises_truncated_on_malformed_upstream_line() {
    // Upstream sends a malformed JSON line (e.g., partial bytes
    // from a buggy producer). The proxy MUST emit a `truncated`
    // SSE event identifying the parse failure and close the stream.
    // Any further bytes from upstream are dropped — once we've
    // truncated, the client is expected to refetch state via
    // `GET /jobs/:id` and decide what to do.
    let mut body = String::new();
    body.push_str(
        r#"{"type":"frame","frame":{"seq":0,"ts_us":0,"level":"info","message":"good"}}"#,
    );
    body.push('\n');
    body.push_str("not-json-at-all\n");
    body.push_str(&ndjson_line(&WireMessage::Terminal {
        outcome: JobOutcome::Passed,
    }));
    let body: &'static str = Box::leak(body.into_boxed_str());

    let (paavod_addr, _g) = spawn_fake_paavod(body).await;
    let (_d, app) = paavo_web_router(paavod_addr);
    let sse = fetch_sse_body(app, "/api/jobs/01ARZ3NDEKTSV4RRFFQ69G5FAV/stream").await;

    assert!(
        sse.contains("event: frame\n"),
        "first (valid) frame should have been emitted; body:\n{sse}"
    );
    assert!(
        sse.contains("event: truncated\n"),
        "malformed line did not surface as truncated; body:\n{sse}"
    );
    assert!(
        sse.contains(r#""reason":"malformed upstream frame"#),
        "wrong truncated reason; body:\n{sse}"
    );
    // The proxy MUST stop after emitting truncated — the legitimate
    // terminal that comes after the malformed line should NOT be
    // forwarded (the upstream state is now uncertain by definition).
    assert!(
        !sse.contains("event: terminal\n"),
        "proxy continued past truncated; body:\n{sse}"
    );
}

#[tokio::test]
async fn since_seq_filters_historicals_from_sse_stream() {
    // Upstream replays frames seq 1..=10 then a terminal. A viewer that
    // already rendered through seq 5 (SSR) reconnects with ?since_seq=5;
    // the proxy must drop frames 1..=5 and emit only 6..=10 + terminal.
    let mut body = String::new();
    for seq in 1..=10u64 {
        body.push_str(&ndjson_line(&WireMessage::Frame {
            frame: LogFrame {
                seq,
                ts_us: seq * 1000,
                level: LogLevel::Info,
                // Token chosen so no message is a substring of another
                // (e.g. "line 1" is a substring of "line 10"): the
                // trailing "X" makes "L1X" not a substring of "L10X".
                target: None,
                message: format!("L{seq}X"),
            },
        }));
    }
    body.push_str(&ndjson_line(&WireMessage::Terminal {
        outcome: JobOutcome::Passed,
    }));
    // spawn_fake_paavod takes &'static str; leak the body (test-only).
    let body: &'static str = Box::leak(body.into_boxed_str());

    let (addr, _g) = spawn_fake_paavod(body).await;
    let (_dir, app) = paavo_web_router(addr);
    let sse = fetch_sse_body(
        app,
        "/api/jobs/01ARZ3NDEKTSV4RRFFQ69G5FAV/stream?since_seq=5",
    )
    .await;

    for seq in 1..=5u64 {
        assert!(
            !sse.contains(&format!("L{seq}X")),
            "frame {seq} should have been filtered; body:\n{sse}"
        );
    }
    for seq in 6..=10u64 {
        assert!(
            sse.contains(&format!("L{seq}X")),
            "frame {seq} should have passed through; body:\n{sse}"
        );
    }
    assert!(
        sse.contains("event: terminal\n"),
        "terminal must still pass through; body:\n{sse}"
    );
}
