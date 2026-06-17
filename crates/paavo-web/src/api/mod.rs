//! JSON API surface consumed by the WASM SPA (`paavo-web-ui`).
//!
//! Every handler here returns `Json<T>` (or a `(StatusCode, String)`
//! error tuple) — there is no server-rendered HTML in this crate any
//! more; the browser fetches data from these endpoints and renders it
//! client-side. The live-update spine is `events` (a single
//! consolidated SSE), while `jobs`/`boards`/`schedules` are the
//! paginated read endpoints. The per-job log proxy lives in
//! [`crate::proxy`] (it bridges paavod's NDJSON, so it is not a plain
//! DB read like the rest of this module).
pub mod boards;
pub mod dashboard;
pub mod events;
pub mod jobs;
pub mod schedules;
