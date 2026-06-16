//! Wire shape for `GET /jobs/:id/stream` NDJSON lines.
//!
//! Producer: `paavod::routes::jobs::stream_job` serialises one
//! [`WireMessage`] per NDJSON line via `serde_json::to_string`.
//!
//! Consumers: `paavo-cli` (`cmd_run.rs`, `cmd_jobs.rs`) and the
//! future `paavo-web` SSE proxy both deserialise each line into
//! [`WireMessage`] and `match` exhaustively.
//!
//! ## Versioning
//!
//! This enum is **additive only**. New variants MUST be tagged with
//! new `type` strings; existing variants MUST NOT change field names
//! or shapes. Consumers MUST treat unknown `type` values as ignorable
//! (forward-compat shim for clients pinned to an older `paavo-proto`
//! than the daemon — `serde_json::from_str::<WireMessage>` returning
//! `Err` is the signal to fall through to "unknown line, log and
//! continue").
//!
//! ## Byte-level wire compatibility with the historical hand-rolled JSON
//!
//! The four pre-existing variants (`Frame`, `Terminal`, `Lagged`,
//! `Truncated`) emit byte-identical JSON to the prior
//! `serde_json::json!({"type":"frame","frame": ...})` macro form,
//! verified by `tests/wire_compat.rs`. Two preconditions for that
//! invariant:
//!
//! - **All variants are struct variants** (`{ frame: LogFrame }`,
//!   never `(LogFrame)`). Internal tagging on tuple variants would
//!   flatten the tuple's content into the outer object alongside
//!   `"type"`, breaking `paavo-cli`'s `v["frame"]["message"]` lookup.
//! - **Serde writes object fields in declaration order**. The macro
//!   form's `{"type":"frame","frame": ...}` has `type` before
//!   `frame`; `#[serde(tag = "type")]` synthesises the same `type`
//!   field first. Reordering the variant body would shift `frame`
//!   before `type` on the wire — so the variants below keep the
//!   single struct field as the only declared field, matching the
//!   macro layout.

use crate::{JobOutcome, LogFrame};
use serde::{Deserialize, Serialize};

/// Non-terminal phase of a job's lifecycle.
///
/// Terminal phases are represented by [`WireMessage::Terminal`]; this
/// enum covers only the in-flight transitions a stream subscriber
/// observes between subscribe and terminal. paavod publishes a
/// [`WireMessage::Phase`] event synchronously with each non-terminal
/// state transition (Submitted→Building→Running) so live viewers can
/// update a phase indicator without polling `GET /jobs/:id`.
///
/// Phase events are NOT persisted to `log_frame`; they're a
/// stream-only signal. Historical replay reconstructs phase from
/// `JobView.{state, started_at, finished_at}` plus per-frame `target`
/// prefixes (`cargo:*` lines came from build, defmt module paths from
/// run).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobPhase {
    /// `paavo-build` is compiling. Frames during this phase carry
    /// `target` of `cargo:stdout` or `cargo:stderr`.
    Building,
    /// `paavo-runner` is attached to a probe and the test ELF is
    /// executing. Frames during this phase carry defmt `target`s
    /// (Rust module paths, e.g. `app::dma`) or `None`.
    Running,
}

/// One NDJSON line on the `/jobs/:id/stream` long-poll body.
///
/// Wire format: a JSON object with a `type` tag chosen via
/// `#[serde(tag = "type")]`. Every variant is a struct variant
/// (no tuple variants) so that serde's internal tagging produces
/// exactly the historical hand-rolled JSON shape; see the variant
/// docs for byte-level examples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireMessage {
    /// One log frame (defmt frame from a running test, or a build
    /// line emitted by paavo-build's streaming refactor — both flow
    /// through the same `LogFrame` shape, distinguished by
    /// `frame.target`).
    ///
    /// Wire example:
    /// ```json
    /// {"type":"frame","frame":{"seq":42,"ts_us":12345,"level":"info","target":"cargo:stderr","message":"   Compiling foo v0.1.0"}}
    /// ```
    /// `frame.target` is omitted from the JSON if `None` (per
    /// `LogFrame`'s own `skip_serializing_if`).
    Frame {
        /// The frame.
        frame: LogFrame,
    },

    /// Terminal outcome — the stream closes after this line.
    ///
    /// Emitted exactly once on the happy path, or replaced by
    /// [`WireMessage::Truncated`] on a degraded close.
    ///
    /// Wire examples:
    /// ```json
    /// {"type":"terminal","outcome":"passed"}
    /// {"type":"terminal","outcome":{"failed":{"kind":"build_err","stderr":"error[E0425]: cannot find value `foo`"}}}
    /// {"type":"terminal","outcome":{"timed_out":{"reason":"inactivity","elapsed_ms":120000}}}
    /// {"type":"terminal","outcome":{"aborted":{"by":"user"}}}
    /// ```
    Terminal {
        /// Outcome — see [`JobOutcome`] for the inner shape (which is
        /// itself externally-tagged).
        outcome: JobOutcome,
    },

    /// Live broadcast channel dropped frames; client should refetch
    /// from the historical endpoint (or re-page `/jobs/:id/stream`)
    /// to recover.
    ///
    /// Wire example: `{"type":"lagged","missed":7}`
    Lagged {
        /// Number of frames the broadcast channel evicted.
        missed: u64,
    },

    /// Stream ended without a terminal line. Client should re-query
    /// `GET /jobs/:id` for the authoritative state.
    ///
    /// Wire example: `{"type":"truncated","reason":"db error reading historical frames"}`
    Truncated {
        /// Operator-facing description.
        reason: String,
    },

    /// Job lifecycle phase transition. Published synchronously with
    /// the corresponding non-terminal DB state transition by
    /// `paavod::dispatch`. **NEW since the build-log streaming
    /// refactor** — older `paavo-cli` versions ignore unknown `type`
    /// values gracefully (see the module-level "Versioning" note).
    ///
    /// Wire example: `{"type":"phase","phase":"running"}`
    Phase {
        /// Which phase was just entered.
        phase: JobPhase,
    },
}

impl WireMessage {
    /// True for variants that close the stream. Used by consumers
    /// (paavo-cli, paavo-web's SSE proxy) to know when to break the
    /// receive loop without having to remember which variants are
    /// terminal-equivalent.
    pub fn closes_stream(&self) -> bool {
        matches!(
            self,
            WireMessage::Terminal { .. } | WireMessage::Truncated { .. }
        )
    }
}
