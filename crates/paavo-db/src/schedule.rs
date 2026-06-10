//! schedule table helpers (filled in by paavod cron wiring).

/// Row representation of the `schedule` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRow;

/// Partial-update payload used when the cron driver fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleUpdate;
