//! Timestamp formatters for paavo-web pages.
//!
//! Centralises every conversion from raw `i64` epoch-milliseconds (the
//! shape paavo-db stores in board / job / schedule rows) and from
//! `LogFrame::ts_us` (microseconds-since-job-start, NOT epoch — see
//! `paavo_proto::LogFrame::ts_us` rustdoc) into operator-facing strings.
//!
//! Pulling these into one module keeps the format consistent across
//! pages and is the seam future commits will reuse — the SSE proxy
//! pre-formats `ts_us` server-side and emits a `display_ts` field, the
//! eventual maud refactor calls the same helpers, etc.
//!
//! ## Conventions
//!
//! - Wall-clock display uses **UTC** (no per-server timezone surprise
//!   when an operator screenshots a log and pastes it in chat). Local
//!   time would diverge across a multi-host paavo deployment; UTC is
//!   the schema-of-record so we render in the schema-of-record.
//! - Format string is `"%Y-%m-%d %H:%M:%S UTC"` — short, sortable,
//!   unambiguous (no day/month-first confusion).
//! - Relative-to-now ("3 minutes ago") is rendered in parentheses
//!   AFTER the absolute time so screenshots stay self-describing.
//! - Microsecond `ts_us` for log frames renders as `mm:ss.fff` (zero-
//!   padded). For long jobs (≥ 1 hour) it grows to `H:MM:SS.fff`. The
//!   millisecond truncation matches the resolution embedded operators
//!   actually care about; sub-millisecond detail is in the raw `ts_us`
//!   field on the wire if anyone needs it.

use chrono::{DateTime, TimeZone, Utc};

/// Format an epoch-millisecond timestamp as `"YYYY-MM-DD HH:MM:SS UTC"`.
/// Returns `"—"` when the input is `None` (the dash matches the prior
/// rendering of `Option<i64>::None` so any operator screenshots stay
/// readable across the upgrade).
pub fn epoch_ms_to_utc(t_ms: Option<i64>) -> String {
    match t_ms {
        Some(ms) => match Utc.timestamp_millis_opt(ms).single() {
            Some(dt) => dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            // Out-of-range epoch ms (negative or post-9999) — surface
            // verbatim so the corruption doesn't get hidden behind a
            // dash. Doesn't happen with real paavod data, but if a
            // future migration writes a sentinel like i64::MIN this
            // tells the operator something's wrong.
            None => format!("invalid epoch ms: {ms}"),
        },
        None => "—".to_string(),
    }
}

/// As [`epoch_ms_to_utc`] but adds a parenthesised relative companion
/// (e.g. `"2026-06-15 18:21:45 UTC (3 minutes ago)"`). Used on the
/// dashboard / boards / schedule pages where the operator wants to
/// glance "how recent is this" without doing date arithmetic.
///
/// `now_ms` is taken as a parameter (instead of calling `Utc::now()`
/// internally) so renders are deterministic in tests. Callers pass
/// `Utc::now().timestamp_millis()` from inside the `axum` handler.
pub fn epoch_ms_with_relative(t_ms: Option<i64>, now_ms: i64) -> String {
    match t_ms {
        Some(ms) => {
            let abs = epoch_ms_to_utc(Some(ms));
            let rel = relative_to_now(ms, now_ms);
            format!("{abs} ({rel})")
        }
        None => "—".to_string(),
    }
}

/// "3 minutes ago" / "in 2 hours" — relative to `now_ms`. Bucketed at
/// natural-sounding boundaries: seconds → minutes → hours → days →
/// "YYYY-MM-DD" once it's over a year so the relative form stops
/// pretending to be useful.
///
/// Future and past are both supported; future renders as `"in N <unit>"`
/// and past as `"N <unit> ago"`. Schedules call this with `next-fire`
/// timestamps in the future; everything else is past.
pub fn relative_to_now(t_ms: i64, now_ms: i64) -> String {
    let delta_ms = now_ms - t_ms;
    let abs_ms = delta_ms.unsigned_abs();
    let in_future = delta_ms < 0;

    // Pick the largest unit that fits, then pluralise. The sequence is
    // chosen for round-number-friendliness, not strict SI: 60 s in a
    // minute, 60 min in an hour, 24 h in a day, 7 days in a week, ~30
    // days in a month, 365 days in a year.
    let (n, unit) = if abs_ms < 1_000 {
        (abs_ms, "millisecond")
    } else if abs_ms < 60_000 {
        (abs_ms / 1_000, "second")
    } else if abs_ms < 3_600_000 {
        (abs_ms / 60_000, "minute")
    } else if abs_ms < 86_400_000 {
        (abs_ms / 3_600_000, "hour")
    } else if abs_ms < 7 * 86_400_000 {
        (abs_ms / 86_400_000, "day")
    } else if abs_ms < 30 * 86_400_000 {
        (abs_ms / (7 * 86_400_000), "week")
    } else if abs_ms < 365 * 86_400_000 {
        (abs_ms / (30 * 86_400_000), "month")
    } else {
        // Once we're more than a year out, the "N years ago" / "in N
        // years" framing stops being useful — operators want the
        // actual date. Fall through to the absolute renderer.
        return epoch_ms_to_utc(Some(t_ms));
    };

    let unit = if n == 1 {
        unit.to_string()
    } else {
        format!("{unit}s")
    };
    if in_future {
        format!("in {n} {unit}")
    } else {
        format!("{n} {unit} ago")
    }
}

/// Format a `LogFrame.ts_us` (microseconds since job start) as
/// `"mm:ss.fff"` for typical jobs and `"H:MM:SS.fff"` for jobs that
/// run past one hour. Returns `"     0:00.000"`-padded width 9 by
/// default so log lines align in a `<pre>` block; pass
/// `pad_to_width = false` to drop the leading spaces (useful for
/// inline rendering in tooltips).
///
/// Resolution is millisecond — sub-millisecond detail is dropped to
/// keep the string short. The raw `ts_us` is still on the wire and in
/// the DB row for anyone who needs nanosecond-precision interleaving.
pub fn relative_us(ts_us: u64, pad_to_width: bool) -> String {
    let total_ms = ts_us / 1_000;
    let ms = total_ms % 1_000;
    let total_s = total_ms / 1_000;
    let s = total_s % 60;
    let total_min = total_s / 60;
    let m = total_min % 60;
    let h = total_min / 60;

    let core = if h == 0 {
        format!("{m:02}:{s:02}.{ms:03}")
    } else {
        format!("{h}:{m:02}:{s:02}.{ms:03}")
    };
    if pad_to_width && h == 0 {
        // mm:ss.fff is 9 chars; longer-form takes its own width.
        format!("{core:>9}")
    } else {
        core
    }
}

/// Format a `LogFrame.ts_us` for absolute display, given the job's
/// submission timestamp in epoch ms. Used for tooltips on log lines so
/// an operator hovering over `00:45.123` sees the actual wall-clock
/// time the frame was emitted.
///
/// Returns `None` when the job hasn't been submitted yet (impossible
/// for a real LogFrame to exist in that state, but the API stays
/// total).
pub fn ts_us_to_wall_clock(ts_us: u64, submitted_at_ms: Option<i64>) -> Option<String> {
    let submitted_at_ms = submitted_at_ms?;
    let frame_ms = ts_us / 1_000;
    // Saturating add so a hypothetical billion-microsecond ts_us can't
    // wrap. The cast is safe up to ~292 million years of uptime.
    let ms = (submitted_at_ms as i128) + (frame_ms as i128);
    let ms = i64::try_from(ms).ok()?;
    let dt: DateTime<Utc> = Utc.timestamp_millis_opt(ms).single()?;
    Some(dt.format("%Y-%m-%d %H:%M:%S%.3f UTC").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_ms_to_utc_known_value() {
        // 1735689600000 = 2025-01-01 00:00:00 UTC (a stable, easily
        // reasoned-about epoch ms).
        assert_eq!(
            epoch_ms_to_utc(Some(1_735_689_600_000)),
            "2025-01-01 00:00:00 UTC"
        );
    }

    #[test]
    fn epoch_ms_to_utc_none_renders_dash() {
        assert_eq!(epoch_ms_to_utc(None), "—");
    }

    #[test]
    fn relative_to_now_buckets() {
        let now = 1_000_000_000_000;
        // exact-minute boundary
        assert_eq!(relative_to_now(now - 60_000, now), "1 minute ago");
        assert_eq!(relative_to_now(now - 120_000, now), "2 minutes ago");
        // exact-hour boundary
        assert_eq!(relative_to_now(now - 3_600_000, now), "1 hour ago");
        assert_eq!(relative_to_now(now - 7_200_000, now), "2 hours ago");
        // day
        assert_eq!(relative_to_now(now - 86_400_000, now), "1 day ago");
        assert_eq!(relative_to_now(now - 5 * 86_400_000, now), "5 days ago");
        // week
        assert_eq!(relative_to_now(now - 7 * 86_400_000, now), "1 week ago");
        // future tense (schedules)
        assert_eq!(relative_to_now(now + 60_000, now), "in 1 minute");
        assert_eq!(relative_to_now(now + 7_200_000, now), "in 2 hours");
    }

    #[test]
    fn relative_to_now_seconds_singular_plural() {
        let now = 1_000_000_000_000;
        assert_eq!(relative_to_now(now - 1_000, now), "1 second ago");
        assert_eq!(relative_to_now(now - 30_000, now), "30 seconds ago");
    }

    #[test]
    fn relative_to_now_falls_back_to_absolute_after_a_year() {
        // > 365 days delta → absolute UTC string.
        let now = 1_735_689_600_000;
        let two_years_ago = now - (2 * 365 * 86_400_000);
        assert_eq!(relative_to_now(two_years_ago, now), "2023-01-02 00:00:00 UTC");
    }

    #[test]
    fn relative_us_formatting() {
        assert_eq!(relative_us(0, true), "00:00.000");
        assert_eq!(relative_us(45_123_000, true), "00:45.123");
        assert_eq!(relative_us(125_000_000, true), "02:05.000");
        // Hour boundary — switches format and drops mm:ss padding rules.
        assert_eq!(relative_us(3_600_000_000, false), "1:00:00.000");
        assert_eq!(relative_us(7_265_123_000, false), "2:01:05.123");
    }

    #[test]
    fn relative_us_pad_off_drops_leading_space() {
        assert_eq!(relative_us(45_123_000, false), "00:45.123");
    }

    #[test]
    fn ts_us_to_wall_clock_adds_offset() {
        let submitted = Some(1_735_689_600_000); // 2025-01-01 00:00:00 UTC
        // 45.123 seconds into the job
        assert_eq!(
            ts_us_to_wall_clock(45_123_000, submitted),
            Some("2025-01-01 00:00:45.123 UTC".into())
        );
    }

    #[test]
    fn ts_us_to_wall_clock_returns_none_when_unknown_submitted() {
        assert_eq!(ts_us_to_wall_clock(45_123_000, None), None);
    }

    #[test]
    fn epoch_ms_with_relative_includes_both() {
        let t = 1_735_689_600_000;
        let now = t + 60_000;
        assert_eq!(
            epoch_ms_with_relative(Some(t), now),
            "2025-01-01 00:00:00 UTC (1 minute ago)"
        );
    }

    #[test]
    fn epoch_ms_with_relative_handles_none() {
        assert_eq!(epoch_ms_with_relative(None, 1_735_689_600_000), "—");
    }
}
