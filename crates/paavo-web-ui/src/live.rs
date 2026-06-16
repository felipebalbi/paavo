//! Live revision signals fed by the consolidated `/api/events` SSE channel.
//!
//! Filled in by Task 4.3 (live revision signals). Exposes per-resource
//! `RwSignal<u64>` revisions that components read inside a resource source so
//! a server-pushed bump transparently refetches the current view.
