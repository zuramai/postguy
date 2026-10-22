//! The Console model: a bounded, local ring buffer of sends, log lines, warnings
//! and errors. Secrets are masked before the entry exists, so no later consumer can leak them.
//! Bodies are truncated for the log with a note; the Response viewer keeps the full body.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use postguy_util::time::now_unix_ms;
use serde::{Deserialize, Serialize};

/// Severity / filter-chip class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConsoleLevel {
    Info,
    Warning,
    Error,
}

/// What produced the entry (the toolbar's filter chips).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConsoleKind {
    Request,
    Log,
    Warning,
    Error,
}

/// One line in the Console.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleEntry {
    pub id: u64,
    pub time_ms: i64,
    pub level: ConsoleLevel,
    pub kind: ConsoleKind,
    /// One line, already masked. Bodies are truncated with `… (<n> bytes truncated)`.
    pub message: String,
    /// Optional expanded detail (resolved request/response, redirect hops, proxy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Ring buffer: 5000 entries / 24 hours, oldest dropped first.
pub const CONSOLE_CAP: usize = 5000;
/// Age cap in milliseconds.
pub const CONSOLE_MAX_AGE_MS: i64 = 24 * 60 * 60 * 1000;
/// What the log keeps of one body.
pub const LOG_BODY_CAP: usize = 1 << 20;

/// Thread-safe console shared by the engine and every frontend.
#[derive(Debug, Default)]
pub struct ConsoleLog {
    entries: Mutex<Vec<ConsoleEntry>>,
    next_id: AtomicU64,
}

impl ConsoleLog {
    pub fn new() -> ConsoleLog {
        ConsoleLog::default()
    }

    /// Add an entry, masking nothing here (callers pass already-masked text) and pruning by
    /// age and cap. Returns the entry as stored.
    pub fn push(&self, level: ConsoleLevel, kind: ConsoleKind, message: impl Into<String>) -> ConsoleEntry {
        self.push_with_detail(level, kind, message, None)
    }

    pub fn push_with_detail(&self, level: ConsoleLevel, kind: ConsoleKind, message: impl Into<String>, detail: Option<String>) -> ConsoleEntry {
        let entry = ConsoleEntry {
            id: self.next_id.fetch_add(1, Ordering::Relaxed).saturating_add(1),
            time_ms: now_unix_ms(),
            level,
            kind,
            message: message.into(),
            detail,
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.push(entry.clone());
            let cutoff = now_unix_ms().saturating_sub(CONSOLE_MAX_AGE_MS);
            let first_live = entries.iter().position(|entry| entry.time_ms >= cutoff).unwrap_or(entries.len());
            if first_live > 0 {
                entries.drain(..first_live);
            }
            if entries.len() > CONSOLE_CAP {
                let drop_count = entries.len().saturating_sub(CONSOLE_CAP);
                entries.drain(..drop_count);
            }
        }
        entry
    }

    /// Newest last. `limit == 0` means everything retained.
    pub fn entries(&self, limit: usize) -> Vec<ConsoleEntry> {
        let Ok(entries) = self.entries.lock() else { return Vec::new() };
        if limit == 0 || entries.len() <= limit {
            return entries.clone();
        }
        let start = entries.len().saturating_sub(limit);
        entries.get(start..).map(<[ConsoleEntry]>::to_vec).unwrap_or_default()
    }

    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|entries| entries.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Truncate a body for a Console detail, noting exactly how much was left out.
pub fn truncate_for_log(bytes: &[u8]) -> String {
    let shown = bytes.get(..LOG_BODY_CAP).unwrap_or(bytes);
    let text = String::from_utf8_lossy(shown);
    if bytes.len() > LOG_BODY_CAP { format!("{text}… ({} bytes truncated)", bytes.len().saturating_sub(LOG_BODY_CAP)) } else { text.into_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_numbered_and_ordered_oldest_first() {
        let console = ConsoleLog::new();
        console.push(ConsoleLevel::Info, ConsoleKind::Log, "first");
        console.push(ConsoleLevel::Error, ConsoleKind::Error, "second");
        let entries = console.entries(0);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].id < entries[1].id);
        assert_eq!(entries[1].level, ConsoleLevel::Error);
    }

    #[test]
    fn the_cap_drops_the_oldest_and_limit_returns_the_newest() {
        let console = ConsoleLog::new();
        for index in 0..(CONSOLE_CAP + 5) {
            console.push(ConsoleLevel::Info, ConsoleKind::Log, format!("line {index}"));
        }
        assert_eq!(console.len(), CONSOLE_CAP);
        let newest = console.entries(3);
        assert_eq!(newest.len(), 3);
        assert_eq!(newest.last().map(|entry| entry.message.as_str()), Some(format!("line {}", CONSOLE_CAP + 4).as_str()));
        console.clear();
        assert!(console.is_empty());
    }

    #[test]
    fn log_bodies_are_truncated_with_a_note() {
        let long = vec![b'x'; LOG_BODY_CAP + 42];
        let logged = truncate_for_log(&long);
        assert!(logged.ends_with("… (42 bytes truncated)"));
        assert_eq!(truncate_for_log(b"short"), "short");
    }
}
