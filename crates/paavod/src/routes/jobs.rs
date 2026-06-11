//! /jobs/* handlers.

use crate::app_state::AppState;
use crate::state_dir::StateDir;
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use paavo_core::{enqueue_job, validate_enqueue, EnqueueRequest};
use paavo_proto::{BoardSelector, JobId, JobSource, JobView, Priority};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::{error, warn};

/// JSON metadata part on `POST /jobs`. `source` is NOT here — every
/// HTTP submit is recorded as `JobSource::Cli`; the scheduler reaches
/// `enqueue_job` directly. Unknown fields are rejected with 400 so
/// the wire schema is unambiguous.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostJobMetadata {
    /// Scheduler priority.
    pub priority: Priority,
    /// Free text id; no auth.
    pub submitter: String,
    /// Selector.
    pub board_selector: BoardSelector,
    /// Optional inactivity override (ms). Defaults to
    /// `timeouts.default_inactivity_s * 1000`.
    #[serde(default)]
    pub inactivity_timeout_ms: Option<u64>,
    /// Optional hard-max override (ms). Defaults to
    /// `timeouts.default_ad_hoc_hard_max_s * 1000`.
    #[serde(default)]
    pub hard_max_ms: Option<u64>,
}

/// 202 response body.
#[derive(Debug, Serialize)]
pub struct AcceptedBody {
    /// Newly assigned job id.
    pub job_id: String,
}

type HandlerResult<T> = Result<T, (StatusCode, String)>;

/// POST /jobs.
pub async fn post_jobs(
    State(s): State<AppState>,
    mut multipart: Multipart,
) -> HandlerResult<(StatusCode, Json<AcceptedBody>)> {
    if s.drain.is_draining() {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "paavod is draining".into()));
    }

    let job_id = JobId::new();
    let sd = StateDir::from_root(&s.config.server.state_dir);
    sd.ensure_dirs()
        .map_err(|e| internal("ensure_dirs", e.to_string()))?;

    // Reserve a temp file path under uploads/; the JobId disambiguates
    // concurrent uploaders. We stream the `crate` part directly into
    // this file, hashing with blake3 in flight, then atomically rename
    // to `<blake>.tar` after validation succeeds.
    let tmp_path = sd.uploads_dir.join(format!(".tmp-{job_id}.tar"));
    // Guard so any early return (validation failure, multipart error,
    // mid-stream I/O fault) unlinks the temp file.
    let mut cleanup = TempCleanup::new(tmp_path.clone());

    let mut metadata: Option<PostJobMetadata> = None;
    let mut crate_seen = false;
    let mut hasher = blake3::Hasher::new();

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(multipart_err("multipart"))?
    {
        match field.name() {
            Some("metadata") => {
                if metadata.is_some() {
                    return Err((StatusCode::BAD_REQUEST, "duplicate `metadata` part".into()));
                }
                // Per-field cap: a malicious client could otherwise burn
                // up to `max_upload_bytes` of RAM on a single JSON part.
                // Legitimate metadata is a few hundred bytes.
                const METADATA_MAX_BYTES: usize = 64 * 1024;
                let mut buf: Vec<u8> = Vec::new();
                while let Some(chunk) = field.chunk().await.map_err(multipart_err("metadata"))? {
                    if buf.len().saturating_add(chunk.len()) > METADATA_MAX_BYTES {
                        return Err((
                            StatusCode::PAYLOAD_TOO_LARGE,
                            format!("metadata field exceeds {METADATA_MAX_BYTES} byte cap"),
                        ));
                    }
                    buf.extend_from_slice(&chunk);
                }
                let parsed: PostJobMetadata =
                    serde_json::from_slice(&buf).map_err(bad_request("metadata"))?;
                metadata = Some(parsed);
            }
            Some("crate") => {
                if crate_seen {
                    return Err((StatusCode::BAD_REQUEST, "duplicate `crate` part".into()));
                }
                crate_seen = true;
                let mut file = tokio::fs::File::create(&tmp_path)
                    .await
                    .map_err(|e| internal("create tmp", e.to_string()))?;
                while let Some(chunk) = field.chunk().await.map_err(multipart_err("crate"))? {
                    hasher.update(&chunk);
                    file.write_all(&chunk)
                        .await
                        .map_err(|e| internal("write tmp", e.to_string()))?;
                }
                file.flush()
                    .await
                    .map_err(|e| internal("flush tmp", e.to_string()))?;
            }
            _ => {} // ignore unknown fields silently
        }
    }

    let metadata = metadata.ok_or((StatusCode::BAD_REQUEST, "missing metadata part".into()))?;
    if !crate_seen {
        return Err((StatusCode::BAD_REQUEST, "missing crate part".into()));
    }
    let blake = hasher.finalize().to_hex().to_string();

    // Resolve defaults from config.
    let tcfg = &s.config.timeouts;
    let inactivity_timeout_ms = metadata
        .inactivity_timeout_ms
        .unwrap_or(tcfg.default_inactivity_s * 1_000);
    let hard_max_ms = metadata
        .hard_max_ms
        .unwrap_or(tcfg.default_ad_hoc_hard_max_s * 1_000);
    let daemon_ceiling_ms = tcfg.daemon_ceiling_s * 1_000;

    // Validate selector + ceiling BEFORE rename so a 400 leaves no
    // orphan tar on disk. Inventory snapshot here is informational —
    // the authoritative check runs inside `enqueue_job` under the db
    // lock with a fresh snapshot. The race (board appears/disappears
    // between this check and the enqueue) is acceptable: at worst a
    // valid submit gets a false 400, or a fail-fast 400 slips through
    // and the authoritative check rejects it with the same error class.
    let pre_req = EnqueueRequest {
        job_id,
        priority: metadata.priority,
        submitter: metadata.submitter.clone(),
        // Server forces source = Cli. The wire schema rejects the field
        // entirely (deny_unknown_fields), but we override here too as
        // defense in depth.
        source: JobSource::Cli,
        board_selector: metadata.board_selector.clone(),
        inactivity_timeout_ms,
        hard_max_ms,
        tar_blake3: String::new(),
        tar_path: String::new(),
        daemon_ceiling_ms,
    };
    {
        let inventory = s.inventory_snapshot();
        validate_enqueue(&pre_req, &inventory).map_err(core_to_http)?;
    }

    // Atomically rename .tmp-<jobid>.tar → <blake>.tar.
    //
    // On a dedup hit (`<blake>.tar` already exists) we deliberately
    // leave `cleanup` armed so its Drop unlinks our temp; the existing
    // copy is content-identical and keeps the build cache warm.
    //
    // On both POSIX (`rename(2)`) and Windows (`MoveFileExW` with
    // REPLACE_EXISTING) `tokio::fs::rename` silently clobbers an
    // existing destination, so two concurrent submitters of identical
    // content both succeed; the loser's temp is gone (renamed) and the
    // winner's bytes are content-identical to whatever was clobbered.
    // We therefore do NOT branch on `AlreadyExists` here.
    //
    // No `fsync`: the build cache is allowed to lose tars on a hard
    // crash — on recovery any DB row whose `tar_path` is missing or
    // partial is treated as a fresh blake3 miss and the next submit
    // re-populates it. Crash-consistency is a non-goal for `uploads/`.
    let final_path = sd.uploads_dir.join(format!("{blake}.tar"));
    let final_path_str = path_to_utf8(&final_path)?;
    if !final_path.is_file() {
        tokio::fs::rename(&tmp_path, &final_path)
            .await
            .map_err(|e| internal("rename tmp", e.to_string()))?;
        cleanup.disarm();
    }
    // (else: dedup hit — leave `cleanup` armed; Drop unlinks the temp.)

    // Authoritative enqueue: take a fresh inventory snapshot under the
    // same db lock so there's no TOCTOU between the snapshot and the
    // insert.
    let now_ms = Utc::now().timestamp_millis();
    let req = EnqueueRequest {
        job_id,
        priority: pre_req.priority,
        submitter: pre_req.submitter,
        source: JobSource::Cli,
        board_selector: pre_req.board_selector,
        inactivity_timeout_ms,
        hard_max_ms,
        tar_blake3: blake,
        tar_path: final_path_str,
        daemon_ceiling_ms,
    };
    let inserted = {
        let db = s.db.lock();
        let inventory = s.inventory.lock().clone();
        enqueue_job(db.raw_conn(), &inventory, req, now_ms).map_err(core_to_http)?
    };
    Ok((
        StatusCode::ACCEPTED,
        Json(AcceptedBody {
            job_id: inserted.to_string(),
        }),
    ))
}

/// RAII guard that unlinks `path` on drop unless `disarm` was called.
/// Used to clean up the streaming temp file on any early-return error
/// path (validation failure, multipart error, mid-stream I/O fault).
struct TempCleanup {
    path: Option<std::path::PathBuf>,
}

impl TempCleanup {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path: Some(path) }
    }
    /// Mark the temp file as handled (renamed away). The Drop impl
    /// becomes a no-op.
    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TempCleanup {
    fn drop(&mut self) {
        if let Some(p) = self.path.take() {
            if let Err(e) = std::fs::remove_file(&p) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!(
                        path = %p.display(),
                        error = %e,
                        "failed to clean up temp upload"
                    );
                }
            }
        }
    }
}

fn bad_request<E: std::fmt::Display>(
    stage: &'static str,
) -> impl FnOnce(E) -> (StatusCode, String) {
    move |e| (StatusCode::BAD_REQUEST, format!("{stage}: {e}"))
}

/// Map an `axum::extract::multipart::MultipartError` to an HTTP status.
/// The error type carries the right status itself — most notably
/// `413 Payload Too Large` when the request exceeds the per-route
/// `DefaultBodyLimit`. Defaulting everything to 400 would hide that.
fn multipart_err(
    stage: &'static str,
) -> impl FnOnce(axum::extract::multipart::MultipartError) -> (StatusCode, String) {
    move |e| (e.status(), format!("{stage}: {e}"))
}

fn internal(stage: &'static str, msg: String) -> (StatusCode, String) {
    error!(stage, msg = %msg, "post_jobs internal error");
    (StatusCode::INTERNAL_SERVER_ERROR, msg)
}

fn path_to_utf8(p: &std::path::Path) -> HandlerResult<String> {
    p.to_str().map(|s| s.to_string()).ok_or_else(|| {
        internal(
            "path_to_utf8",
            format!("non-UTF-8 upload path: {}", p.display()),
        )
    })
}

fn core_to_http(e: paavo_core::CoreError) -> (StatusCode, String) {
    use paavo_core::CoreError::*;
    match e {
        SelectorNeverMatches(_) | OverCeiling { .. } | NotCancellable { .. } => {
            (StatusCode::BAD_REQUEST, format!("{e}"))
        }
        Db(_) | Build(_) | Io(_) => {
            error!(error = %e, "post_jobs internal error");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
        }
    }
}

/// GET /jobs?state=...&limit=... — list jobs filtered by state.
///
/// Defaults: `state` omitted ⇒ `Submitted`, `limit` omitted ⇒ 50,
/// clamped to ≤ 500 to bound response size. Unknown `state` value
/// ⇒ 400. Unparseable or out-of-range `limit` (0 or > 500) ⇒ 400.
/// Returns `Vec<JobView>` — wire shape excludes server-local
/// `tar_path` / `elf_path`.
pub async fn list_jobs(
    State(s): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<JobView>>, (StatusCode, String)> {
    let limit: u32 = match q.get("limit") {
        Some(v) => v
            .parse::<u32>()
            .map_err(|_| (StatusCode::BAD_REQUEST, format!("invalid limit: {v}")))
            .and_then(|n| {
                if (1..=500).contains(&n) {
                    Ok(n)
                } else {
                    Err((
                        StatusCode::BAD_REQUEST,
                        format!("limit must be 1..=500, got {n}"),
                    ))
                }
            })?,
        None => 50,
    };
    let state = match q.get("state") {
        Some(v) => parse_state(v)?,
        None => paavo_proto::JobState::Submitted,
    };
    let rows = {
        let db = s.db.lock();
        paavo_db::JobRow::list_by_state(db.raw_conn(), state, limit)
    }
    .map_err(db_to_http)?;
    Ok(Json(rows.into_iter().map(row_to_view).collect()))
}

// Keep `parse_state` in sync with `paavo_proto::JobState`'s
// `#[serde(rename = "...")]` table; a rename there must propagate
// here.
fn parse_state(s: &str) -> Result<paavo_proto::JobState, (StatusCode, String)> {
    use paavo_proto::JobState::*;
    Ok(match s {
        "submitted" => Submitted,
        "building" => Building,
        "running" => Running,
        "passed" => Passed,
        "failed" => Failed,
        "timedout" => TimedOut,
        "aborted" => Aborted,
        _ => return Err((StatusCode::BAD_REQUEST, format!("unknown state: {s}"))),
    })
}

/// GET /jobs/:id — fetch a single job. 404 on unknown id.
pub async fn get_job(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<JobView>, (StatusCode, String)> {
    let id: JobId = id
        .parse()
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid job id".into()))?;
    let row = {
        let db = s.db.lock();
        paavo_db::JobRow::find(db.raw_conn(), &id)
    }
    .map_err(db_to_http)?;
    match row {
        Some(r) => Ok(Json(row_to_view(r))),
        None => Err((StatusCode::NOT_FOUND, "no such job".into())),
    }
}

/// POST /jobs/:id/cancel — cancel a `Submitted` job inline.
///
/// `Building`/`Running` cancellation will land in M4.3 (signal the
/// worker); for now those return 409 so the API is honest. Unknown id
/// returns 404 (paavo-db surfaces `DbError::NotFound { entity: "job" }`
/// from `JobRow::get`, mapped here without pattern-matching on the
/// underlying rusqlite variant).
pub async fn cancel_job(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let id: JobId = match id.parse() {
        Ok(j) => j,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid job id").into_response(),
    };
    let now_ms = Utc::now().timestamp_millis();
    let res = {
        let db = s.db.lock();
        paavo_core::cancel_if_submitted(db.raw_conn(), &id, now_ms)
    };
    match res {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(paavo_core::CoreError::NotCancellable { state }) => (
            StatusCode::CONFLICT,
            format!("not cancellable in state {state:?}"),
        )
            .into_response(),
        Err(paavo_core::CoreError::Db(paavo_db::DbError::NotFound { .. })) => {
            (StatusCode::NOT_FOUND, "no such job").into_response()
        }
        Err(e) => {
            error!(error = %e, "cancel_job internal error");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

/// Convert a `paavo_db::JobRow` into the wire-safe `paavo_proto::JobView`.
/// Drops the server-local `tar_path` / `elf_path` fields; `tar_blake3`
/// is preserved because it is content-addressed and useful to operators.
fn row_to_view(r: paavo_db::JobRow) -> JobView {
    JobView {
        id: r.id,
        priority: r.priority,
        submitter: r.submitter,
        source: r.source,
        board_selector: r.board_selector,
        inactivity_timeout_ms: r.inactivity_timeout_ms,
        hard_max_ms: r.hard_max_ms,
        state: r.state,
        outcome: r.outcome,
        board_id: r.board_id,
        submitted_at: r.submitted_at,
        started_at: r.started_at,
        finished_at: r.finished_at,
        tar_blake3: r.tar_blake3,
    }
}

/// Map a paavo-db error to an HTTP status. `NotFound`/`AlreadyExists`
/// get the typed mapping; everything else is logged and surfaces as
/// 500.
fn db_to_http(err: paavo_db::DbError) -> (StatusCode, String) {
    match err {
        paavo_db::DbError::NotFound { entity, id } => {
            (StatusCode::NOT_FOUND, format!("{entity} not found: {id}"))
        }
        paavo_db::DbError::AlreadyExists { entity, id } => (
            StatusCode::CONFLICT,
            format!("{entity} already exists: {id}"),
        ),
        other => {
            error!(error = ?other, "unexpected db error");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{other}"))
        }
    }
}

/// GET /jobs/:id/stream — NDJSON long-poll. One JSON object per line:
///   `{"type":"frame","frame":<LogFrame>}` for each historical or live
///   frame, then exactly one `{"type":"terminal","outcome":<JobOutcome>}`
///   line before the stream closes. A `{"type":"lagged","missed":<u64>}`
///   line is emitted if the live broadcast channel falls behind. A
///   `{"type":"truncated","reason":"..."}` line is emitted if the live
///   stream ends without a Terminal event AND the job is still
///   non-terminal in the DB (worker died / unexpected close).
///
/// Response is `application/x-ndjson`. Clients parse by splitting on
/// `\n` and deserializing each line. Frames carry `seq` numbers so
/// clients can dedup if the historical-then-live boundary races (rare
/// in practice — see job_logs.rs for the ordering contract).
///
/// 400 if `:id` is not a valid ULID. 404 if no such job. 500 on db
/// errors. Otherwise 200 with a streaming body.
///
/// Lifecycle ordering: the handler reads the job row FIRST. Only if
/// the job exists AND is non-terminal does it subscribe to the broker.
/// This avoids the Sender-leak class (the unconditional `subscribe`
/// combined with `entry().or_insert_with` would otherwise materialize
/// a Sender for every `/jobs/<random-ulid>/stream` request and never
/// reclaim it). A re-check after subscribe catches the narrow race
/// where the job finalizes between the initial read and the subscribe.
pub async fn stream_job(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::header;
    use axum::response::Response;
    use std::convert::Infallible;

    let id: JobId = match id.parse() {
        Ok(i) => i,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid job id").into_response(),
    };

    // 1. Read the row first to decide whether the job exists and
    //    whether it's already terminal. This is the leak-safe
    //    ordering: we only subscribe to the broker if we actually
    //    need a live channel.
    let (state, outcome) = {
        let db = s.db.lock();
        match paavo_db::JobRow::find(db.raw_conn(), &id) {
            Ok(Some(r)) => (r.state, r.outcome),
            Ok(None) => return (StatusCode::NOT_FOUND, "no such job").into_response(),
            Err(e) => {
                error!(error = %e, "stream_job: db find error");
                return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
        }
    };

    // 2. If terminal, emit historical + terminal-from-DB. No subscribe.
    if state.is_terminal() {
        let Some(outcome) = outcome else {
            // Terminal state with NULL outcome is a corrupted-DB
            // condition (paavo_db::JobRow::finalize requires an
            // OutcomeRecord). Surface 500 rather than fabricate.
            error!(%id, ?state, "stream_job: terminal state with NULL outcome");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "terminal state has no outcome (corrupted DB)",
            )
                .into_response();
        };
        return terminal_response(s.clone(), id, outcome);
    }

    // 3. Subscribe BEFORE re-checking the DB so any frame published in
    //    the gap between subscribe and re-check is captured.
    let rx = s.job_logs.subscribe(id);

    // 4. Re-check: the job may have finalized while we were
    //    subscribing. If so, drop rx and take the terminal-from-DB
    //    branch; finalize the broker entry we just created to avoid
    //    leaking a Sender.
    let (state2, outcome2) = {
        let db = s.db.lock();
        match paavo_db::JobRow::find(db.raw_conn(), &id) {
            Ok(Some(r)) => (r.state, r.outcome),
            Ok(None) => {
                drop(rx);
                s.job_logs.finalize(id);
                return (StatusCode::NOT_FOUND, "no such job").into_response();
            }
            Err(e) => {
                drop(rx);
                s.job_logs.finalize(id);
                error!(error = %e, "stream_job: db recheck error");
                return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
        }
    };
    if state2.is_terminal() {
        drop(rx);
        s.job_logs.finalize(id);
        let Some(outcome) = outcome2 else {
            error!(%id, ?state2, "stream_job: terminal-after-subscribe with NULL outcome");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "terminal state has no outcome (corrupted DB)",
            )
                .into_response();
        };
        return terminal_response(s.clone(), id, outcome);
    }

    // 5. Live: stream paged historical frames, then live frames from
    //    rx until Terminal or rx closes.
    let s_for_stream = s.clone();
    let live = async_stream::stream! {
        // Page historical in chunks so memory stays bounded even for
        // long-running jobs that have emitted >10k frames.
        for line in historical_lines(&s_for_stream, &id) {
            yield Ok::<_, Infallible>(line);
        }
        // Live tail.
        use tokio_stream::wrappers::BroadcastStream;
        use tokio_stream::StreamExt;
        let mut rx = BroadcastStream::new(rx);
        let mut saw_terminal = false;
        while let Some(item) = rx.next().await {
            match item {
                Ok(crate::job_logs::LiveEvent::Frame(f)) => {
                    yield Ok(ndjson_line(&serde_json::json!({"type":"frame","frame": f})));
                }
                Ok(crate::job_logs::LiveEvent::Terminal(o)) => {
                    yield Ok(ndjson_line(
                        &serde_json::json!({"type":"terminal","outcome": o}),
                    ));
                    saw_terminal = true;
                    break;
                }
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                    yield Ok(ndjson_line(&serde_json::json!({
                        "type": "lagged",
                        "missed": n,
                    })));
                }
            }
        }
        if !saw_terminal {
            // Defensive close: rx ended without a Terminal event. Either
            // the worker called finalize() without publishing Terminal
            // (a worker bug), or the lag eviction race ate the Terminal
            // before it reached us. Try to recover the terminal outcome
            // from the DB; if the job is still non-terminal, emit a
            // `truncated` marker so the client knows the stream
            // ended unexpectedly.
            let recovered = {
                let db = s_for_stream.db.lock();
                paavo_db::JobRow::find(db.raw_conn(), &id).ok().flatten()
            };
            match recovered {
                Some(row) if row.state.is_terminal() => {
                    if let Some(o) = row.outcome {
                        yield Ok(ndjson_line(
                            &serde_json::json!({"type":"terminal","outcome": o}),
                        ));
                    } else {
                        yield Ok(ndjson_line(&serde_json::json!({
                            "type": "truncated",
                            "reason": "terminal DB row has NULL outcome",
                        })));
                    }
                }
                _ => {
                    yield Ok(ndjson_line(&serde_json::json!({
                        "type": "truncated",
                        "reason": "live stream ended before terminal",
                    })));
                }
            }
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(live))
        .unwrap()
}

/// Build the terminal-response: paged historical frames + a single
/// `terminal` line. Used when the job is already terminal at handler
/// entry (or becomes terminal during the subscribe race window).
fn terminal_response(
    s: AppState,
    id: JobId,
    outcome: paavo_proto::JobOutcome,
) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::header;
    use axum::response::Response;
    use std::convert::Infallible;

    let live = async_stream::stream! {
        for line in historical_lines(&s, &id) {
            yield Ok::<_, Infallible>(line);
        }
        yield Ok(ndjson_line(
            &serde_json::json!({"type":"terminal","outcome": outcome}),
        ));
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(live))
        .unwrap()
}

/// Page historical frames for `id` in 1000-frame chunks, returning a
/// `Vec` of NDJSON lines (one per frame). DB errors on any page log +
/// produce a `truncated` line so the client sees an explicit failure
/// instead of a silent empty body.
fn historical_lines(s: &AppState, id: &JobId) -> Vec<bytes::Bytes> {
    use paavo_db::LogFrameDb;
    const PAGE: u32 = 1000;
    let mut lines: Vec<bytes::Bytes> = Vec::new();
    let mut offset: u32 = 0;
    loop {
        let chunk_result = {
            let db = s.db.lock();
            paavo_proto::LogFrame::list(db.raw_conn(), id, offset, PAGE)
        };
        match chunk_result {
            Ok(chunk) => {
                let n = chunk.len();
                for f in chunk {
                    lines.push(ndjson_line(&serde_json::json!({"type":"frame","frame": f})));
                }
                if n < PAGE as usize {
                    break;
                }
                offset = offset.saturating_add(PAGE);
            }
            Err(e) => {
                error!(error = %e, %id, "stream_job: db error paging historical frames");
                lines.push(ndjson_line(&serde_json::json!({
                    "type": "truncated",
                    "reason": "db error reading historical frames",
                })));
                break;
            }
        }
    }
    lines
}

fn ndjson_line(v: &serde_json::Value) -> bytes::Bytes {
    let mut s = v.to_string();
    s.push('\n');
    bytes::Bytes::from(s)
}
