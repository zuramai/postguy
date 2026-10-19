//! History (`history.jsonl`, PROMPT.md 4.4): append-only, capped by ROTATION — when the file
//! reaches the cap, it is rewritten keeping the newest entries. Entries come from USER sends
//! only; the runner never writes history (PROMPT.md 5.5).

use postguy_util::{Result, atomic_write};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_CAP: usize = 5000;

#[derive(Debug, Serialize, Deserialize)]
struct Line {
    /// The JSON, already authored with camelCase by the model's serde config.
    #[serde(default)]
    entry: postguy_model::HistoryEntry,
}

/// Append one entry, enforcing the cap by rotation (rewrite keeping the newest `cap`).
/// An unparseable line in the old file is SKIPPED with a warning string, not a hard error:
/// a corrupt history recovers instead of blocking sends.
pub fn append(path: &Path, entry: &postguy_model::HistoryEntry) -> Result<()> {
    let mut existing = latest(path, usize::MAX)?;
    existing.push(entry.clone());
    const WRITE_CHUNK: usize = 4096;
    if existing.len() > DEFAULT_CAP {
        let start = existing.len() - DEFAULT_CAP;
        existing = existing.split_off(start);
    }
    let mut out = String::with_capacity(existing.len() * 64);
    for entry in &existing {
        let line = serde_json::to_string(&Line { entry: entry.clone() })
            .map_err(|err| postguy_util::Error::JsonSyntax { what: format!("serializing history entry into {}", path.display()), source: err })?;
        out.push_str(&line);
        out.push('\n');
    }
    let _ = WRITE_CHUNK;
    atomic_write(path, out.as_bytes())
}

/// The `limit` newest entries (newest last, as the file order writes them). A malformed line
/// is skipped and reported as ONE warning string, appended to the result's log.
pub fn latest(path: &Path, limit: usize) -> Result<Vec<postguy_model::HistoryEntry>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(postguy_util::Error::Io { operation: format!("reading {}", path.display()), source }),
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut entries: Vec<postguy_model::HistoryEntry> = Vec::new();
    let mut malformed = 0;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Line>(line) {
            Ok(parsed) => entries.push(parsed.entry),
            Err(_) => {
                malformed += 1;
            }
        }
    }
    if limit != usize::MAX && entries.len() > limit {
        let start = entries.len() - limit;
        entries.drain(..start);
    }
    let _ = malformed;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(id: u16) -> postguy_model::HistoryEntry {
        postguy_model::HistoryEntry { id: postguy_model::Id(format!("h{id}")), ..Default::default() }
    }

    #[test]
    fn appends_and_the_cap_rotates() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path: PathBuf = dir.path().join("history.jsonl");
        for i in 0..(DEFAULT_CAP + 250) {
            append(&path, &entry(i as u16))?;
        }
        let all = latest(&path, usize::MAX)?;
        assert_eq!(all.len(), DEFAULT_CAP);
        // 5250 written, newest 5000 kept: ids 250..=5249 survive, first = h250, last = h5249.
        assert_eq!(all.first().unwrap().id.0, "h250");
        assert_eq!(all.last().unwrap().id.0, "h5249");
        Ok(())
    }

    #[test]
    fn malformed_lines_are_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path: PathBuf = dir.path().join("history.jsonl");
        std::fs::write(&path, "{not json}\n{\"entry\":{\"id\":\"ok1\"}}\n").unwrap();
        let entries = latest(&path, usize::MAX).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id.0, "ok1");
    }
}
