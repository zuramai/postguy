//! Time helpers, deliberately tiny: the workspace avoids `chrono`. Unix-style millis as the
//! on-disk and UI unit everywhere.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch as i64 (fits the 292-million-year range by far).
pub fn now_unix_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Seconds since the Unix epoch as i64.
pub fn now_unix_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Duration formatting for the UI's compact chips: `1.2 s`, `612 ms`, `3.4 s`, `1:02`.
/// The exact rendering lives in ui-egui later; this is the neutral short form for logs,
/// the console and report builders, theme-independent.
pub fn compact_duration(millis: i64) -> String {
    if millis < 0 {
        return format!("-{}", compact_duration(-millis));
    }
    let millis = millis as u64;
    if millis < 1_000 {
        format!("{millis} ms")
    } else if millis < 60_000 {
        let whole = millis / 1000;
        let tenth = (millis % 1000) / 100;
        if tenth == 0 { format!("{whole} s") } else { format!("{whole}.{tenth} s") }
    } else {
        let minutes = millis / 60_000;
        let seconds = (millis % 60_000) / 1000;
        format!("{minutes}:{seconds:02}")
    }
}

/// How [`format_datetime`] renders an instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateStyle {
    /// `2026-10-26 14:03:12 +07:00`: reports and tooltips.
    Full,
    /// `2026-10-26T14:03:12+07:00`: RFC 3339.
    Iso,
    /// `14:03` today, `24 Oct 14:03` this year, `2025-12-01` before: list rows.
    Compact,
}

/// The system time zone's UTC offset at `unix_ms`, in seconds. Any error (no zone database,
/// out-of-range instant) gives 0, which the formatted text shows as `+00:00`.
pub fn local_offset_seconds(unix_ms: i64) -> i32 {
    let Ok(timestamp) = jiff::Timestamp::from_millisecond(unix_ms) else {
        return 0;
    };
    match jiff::tz::TimeZone::try_system() {
        Ok(zone) => zone.to_offset(timestamp).seconds(),
        Err(_) => 0,
    }
}

/// Format `unix_ms` at the fixed `offset_seconds` (see [`local_offset_seconds`]). `now_ms`
/// only matters for [`DateStyle::Compact`]. Total: an out-of-range input gives `invalid time`.
pub fn format_datetime(unix_ms: i64, offset_seconds: i32, style: DateStyle, now_ms: i64) -> String {
    let Some(local) = civil(unix_ms, offset_seconds) else {
        return "invalid time".to_string();
    };
    let offset = offset_text(offset_seconds);
    match style {
        DateStyle::Full => format!("{} {}", local.strftime("%Y-%m-%d %H:%M:%S"), offset),
        DateStyle::Iso => format!("{}{}", local.strftime("%Y-%m-%dT%H:%M:%S"), offset),
        DateStyle::Compact => {
            let now = civil(now_ms, offset_seconds);
            match now {
                Some(now) if now.date() == local.date() => local.strftime("%H:%M").to_string(),
                Some(now) if now.year() == local.year() => local.strftime("%-d %b %H:%M").to_string(),
                _ => local.strftime("%Y-%m-%d").to_string(),
            }
        }
    }
}

fn civil(unix_ms: i64, offset_seconds: i32) -> Option<jiff::civil::DateTime> {
    let timestamp = jiff::Timestamp::from_millisecond(unix_ms).ok()?;
    let offset = jiff::tz::Offset::from_seconds(offset_seconds).ok()?;
    Some(offset.to_datetime(timestamp))
}

/// `+07:00`, `-03:30`, `+00:00`; seconds are dropped.
fn offset_text(offset_seconds: i32) -> String {
    let sign = if offset_seconds < 0 { '-' } else { '+' };
    let total = offset_seconds.unsigned_abs();
    format!("{sign}{:02}:{:02}", total / 3600, (total % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_monotonically_sane() {
        let a = now_unix_ms();
        let b = now_unix_ms();
        assert!(a > 1_700_000_000_000, "year ≥ 2023");
        assert!(b >= a, "now never goes backwards");
    }

    #[test]
    fn durations_render_like_humans_read_them() {
        assert_eq!(compact_duration(0), "0 ms");
        assert_eq!(compact_duration(612), "612 ms");
        assert_eq!(compact_duration(1_000), "1 s");
        assert_eq!(compact_duration(1_234), "1.2 s");
        assert_eq!(compact_duration(62_000), "1:02");
    }

    #[test]
    fn datetimes_render_in_each_style_and_offset() {
        // 2026-10-26 07:03:12 UTC.
        let at = 1_792_998_192_000;
        assert_eq!(format_datetime(at, 0, DateStyle::Full, at), "2026-10-26 07:03:12 +00:00");
        assert_eq!(format_datetime(at, 25_200, DateStyle::Full, at), "2026-10-26 14:03:12 +07:00");
        assert_eq!(format_datetime(at, 0, DateStyle::Iso, at), "2026-10-26T07:03:12+00:00");
        assert_eq!(format_datetime(at, 25_200, DateStyle::Iso, at), "2026-10-26T14:03:12+07:00");
        assert_eq!(format_datetime(at, -12_600, DateStyle::Full, at), "2026-10-26 03:33:12 -03:30");
        assert_eq!(format_datetime(at, 0, DateStyle::Compact, at + 60_000), "07:03");
        assert_eq!(format_datetime(at, 25_200, DateStyle::Compact, at + 60_000), "14:03");
        let two_days = 2 * 86_400_000;
        assert_eq!(format_datetime(at - two_days, 0, DateStyle::Compact, at), "24 Oct 07:03");
        assert_eq!(format_datetime(at - two_days, 25_200, DateStyle::Compact, at), "24 Oct 14:03");
        let last_year = at - 329 * 86_400_000;
        assert_eq!(format_datetime(last_year, 0, DateStyle::Compact, at), "2025-12-01");
        assert_eq!(format_datetime(last_year, 25_200, DateStyle::Compact, at), "2025-12-01");
        // Today is decided in the given offset: 23:30 UTC is tomorrow at +07:00.
        let late = 1_792_971_000_000; // 2026-10-25 23:30:00 UTC
        assert_eq!(format_datetime(late, 25_200, DateStyle::Compact, at), "06:30");
    }

    #[test]
    fn out_of_range_datetimes_are_reported_not_panicked() {
        assert_eq!(format_datetime(i64::MAX, 0, DateStyle::Full, 0), "invalid time");
        assert_eq!(format_datetime(i64::MIN, 0, DateStyle::Compact, 0), "invalid time");
        assert_eq!(format_datetime(0, i32::MAX, DateStyle::Iso, 0), "invalid time");
        assert_eq!(format_datetime(0, 0, DateStyle::Compact, i64::MAX), "1970-01-01");
        let _ = local_offset_seconds(i64::MAX);
        assert_eq!(local_offset_seconds(i64::MIN), 0);
    }
}
