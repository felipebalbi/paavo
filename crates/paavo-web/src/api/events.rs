//! `GET /api/events` — the single consolidated live-update channel.
//!
//! Rather than one SSE stream per resource, the SPA opens exactly one
//! `EventSource` here. On connect it receives an immediate `snapshot`
//! event carrying the current [`Revisions`] for every resource; from
//! then on it receives one named event (`jobs` / `boards` /
//! `schedules`) per *changed* revision, the data being the new
//! revision number. The client compares that number against the
//! `revision` it last fetched on each list page and refetches only the
//! resources that actually moved. This collapses N polling loops into
//! one push channel and makes reconnects self-healing (the fresh
//! `snapshot` re-syncs whatever was missed while disconnected).
use crate::index::{LiveState, Revisions};
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use std::convert::Infallible;
use std::time::Duration;

/// `GET /api/events` — see the module docs.
///
/// The `watch::Ref` returned by `borrow_and_update()` is copied out
/// (`Revisions: Copy`) and dropped within each statement, so no lock
/// is ever held across the `.await` on `changed()`.
pub async fn events(State(live): State<LiveState>) -> impl IntoResponse {
    let mut rx = live.subscribe();
    let stream = async_stream::stream! {
        let mut prev: Revisions = *rx.borrow_and_update();
        yield Ok::<Event, Infallible>(
            Event::default()
                .event("snapshot")
                .json_data(prev)
                .expect("Revisions always serialises"),
        );
        while rx.changed().await.is_ok() {
            let cur = *rx.borrow_and_update();
            if cur.jobs != prev.jobs {
                yield Ok(Event::default().event("jobs").data(cur.jobs.to_string()));
            }
            if cur.boards != prev.boards {
                yield Ok(Event::default().event("boards").data(cur.boards.to_string()));
            }
            if cur.schedules != prev.schedules {
                yield Ok(Event::default().event("schedules").data(cur.schedules.to_string()));
            }
            prev = cur;
        }
    };
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
