//! Schedule wire view. Mirrors the public fields of paavo-db's
//! `ScheduleRow` (no server-local fields to drop).
use serde::{Deserialize, Serialize};

/// JSON shape for a cron schedule row, served by paavo-web's
/// `GET /api/schedules`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleView {
    /// Schedule id, e.g. `nightly`.
    pub id: String,
    /// Cron expression.
    pub cron: String,
    /// Whether the schedule is active.
    pub enabled: bool,
    /// Last firing time, epoch ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_triggered_at: Option<i64>,
    /// Last completion time, epoch ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completed_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedule_view_roundtrips() {
        let s = ScheduleView {
            id: "nightly".into(),
            cron: "0 0 19 * * *".into(),
            enabled: true,
            last_triggered_at: Some(1),
            last_completed_at: None,
        };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(s, serde_json::from_str::<ScheduleView>(&j).unwrap());
    }
}
