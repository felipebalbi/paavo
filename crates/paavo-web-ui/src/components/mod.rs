//! UI components: the app [`shell`] plus one module per route page
//! ([`dashboard`], [`jobs_list`], [`job_detail`], [`boards`], [`schedule`])
//! and shared [`widgets`]. The page components are placeholders for this
//! shell task; later tasks (4.6–4.10) flesh each one out into its live,
//! paginated table.

pub mod boards;
pub mod dashboard;
pub mod job_detail;
pub mod jobs_list;
pub mod schedule;
pub mod shell;
pub mod widgets;
