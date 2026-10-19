//! Change detection (PROMPT.md 4.4): files edited by another tool are detected by the
//! engine polling this module with a fingerprint (mtime + size) per watched file. The
//! `notify` watcher is a UI-layer nicety added later this is the engine-safe fallback.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Fingerprint of one file (0 for a missing file: disappearance is a change too).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    pub mtime_ms: u128,
    pub size: u64,
}

/// Snapshot of a set of watched files.
#[derive(Debug, Default, Clone)]
pub struct Snapshot(BTreeMap<PathBuf, Fingerprint>);

impl Snapshot {
    /// Fingerprint the given paths (missing files fingerprint as zero).
    pub fn take(paths: impl IntoIterator<Item = PathBuf>) -> Snapshot {
        Snapshot(
            paths
                .into_iter()
                .map(|path| {
                    let print = fingerprint(&path);
                    (path, print)
                })
                .collect(),
        )
    }

    /// Which paths changed (added/removed or altered) since this snapshot?
    pub fn diff_since(&self, previous: &Snapshot) -> Vec<PathBuf> {
        let mut changed: Vec<PathBuf> = Vec::new();
        for (path, fingerprint_now) in &self.0 {
            if previous.0.get(path).is_none_or(|f| f != fingerprint_now) {
                changed.push(path.clone());
            }
        }
        for path in previous.0.keys() {
            if !self.0.contains_key(path) {
                changed.push(path.clone());
            }
        }
        changed
    }
}

/// The fingerprint of one path NOW. A missing file reads `(0, 0)`.
pub fn fingerprint(path: &Path) -> Fingerprint {
    match std::fs::metadata(path) {
        Ok(meta) => Fingerprint { mtime_ms: etime_ms(&meta), size: meta.len() },
        Err(_) => Fingerprint { mtime_ms: 0, size: 0 },
    }
}

fn etime_ms(meta: &std::fs::Metadata) -> u128 {
    meta.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn snapshots_notice_edits_and_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let file: PathBuf = dir.path().join("collection.json");
        std::fs::write(&file, b"one").unwrap();
        let before = Snapshot::take([file.clone()]);
        std::fs::write(&file, b"changed-more").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let after = Snapshot::take([file.clone()]);
        assert_eq!(after.diff_since(&before), vec![file.clone()], "content changed");
        std::fs::remove_file(&file).unwrap();
        let gone = Snapshot::take([file.clone()]);
        assert_eq!(gone.diff_since(&after), vec![file.clone()], "file vanished");
    }
}
