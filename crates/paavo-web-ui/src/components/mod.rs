//! UI components: the app [`shell`] plus one module per route page
//! ([`dashboard`], [`jobs_list`], [`job_detail`], [`boards`], [`schedule`])
//! and shared [`widgets`] (state/health badges, relative-time helpers, and the
//! windowed pagination footer reused by every paginated table).

pub mod boards;
pub mod dashboard;
pub mod job_detail;
pub mod jobs_list;
pub mod schedule;
pub mod shell;
pub mod widgets;
