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
}
