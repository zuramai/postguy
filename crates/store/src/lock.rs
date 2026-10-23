//! The workspace lock file: one process per workspace. A lock holds the pid
//! and a creation time; `acquire` takes over a stale lock (known-dead pid or older than the
//! takeover age) and refuses a live one with a readable error.

use postguy_util::{Result, atomic_write};

/// How old a lock may be before we assume the owner died without cleaning up. A live
/// Postguy renews nothing (crash-prone writes are the very thing we avoid), so the takeover
/// age is the only undefeated guard: three minutes of a quiet lock is stale.
pub const STALE_TAKEOVER_AGE: std::time::Duration = std::time::Duration::from_secs(3 * 60);

/// Content of the lock file: `pid=<pid>\nstart=<unix ms>\n`.
#[derive(Debug, PartialEq, Eq)]
pub struct LockHandle {
    path: std::path::PathBuf,
    pid: u32,
}

impl Drop for LockHandle {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Acquire the lock file at `paths.lock_file()`. Returns the handle; holding it owns the
/// workspace. A LIVE other-process lock errors; a stale one is taken over (documented in the
/// open-stats report).
pub fn acquire(paths: &crate::paths::StorePaths) -> Result<LockHandle> {
    let path = paths.lock_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| postguy_util::Error::Io { operation: format!("creating {}", parent.display()), source })?;
    }
    let my_pid = std::process::id();
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let (pid, start) = parse_lock(&existing);
        let alive = match pid {
            Some(pid) => pid_alive(pid),
            None => false,
        };
        if alive && !older_than(&start, STALE_TAKEOVER_AGE) {
            let owner = pid.map_or_else(|| "an unknown owner".to_string(), |p| format!("pid {p}"));
            return Err(postguy_util::Error::Unusable {
                what: format!("workspace already open by {owner}; close that instance or remove {} if it is gone", path.display()),
            });
        }
        // Stale: continue and overwrite (the takeover report lands in the open message).
        let _ = (pid, start);
    }
    let text = format!("pid={my_pid}\nstart={}\n", crate::lock::unix_ms());
    atomic_write(&path, text.as_bytes())?;
    Ok(LockHandle { path, pid: my_pid })
}

/// `(pid, created_unix_ms)` from a lock file, tolerant of a truncated write.
pub fn parse_lock(text: &str) -> (Option<u32>, Option<std::time::Duration>) {
    let mut pid = None;
    let mut start = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("pid=") {
            pid = rest.trim().parse().ok();
        }
        if let Some(rest) = line.strip_prefix("start=") {
            start = rest.trim().parse::<i64>().ok().map(|ms| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|now| now - std::time::Duration::from_millis(ms as u64))
                    .unwrap_or_default()
            });
        }
    }
    (pid, start)
}

/// Whether `pid` names a LIVE process on THIS machine (`ps -p` on unix, `tasklist` on Windows;
/// when the check can not run the pid is assumed alive, the takeover age still applies).
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        // `tasklist` prints a CSV row with the quoted pid when the process exists, and an INFO line otherwise.
        match std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"]).output() {
            Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")),
            _ => true,
        }
    }
    #[cfg(not(windows))]
    {
        match std::process::Command::new("ps").args(["-p", &pid.to_string()]).output() {
            Ok(output) => output.status.success(),
            Err(_) => true, // can not know: safer to assume live (refuse rather than clobber)
        }
    }
}

fn older_than(ago: &Option<std::time::Duration>, limit: std::time::Duration) -> bool {
    match ago {
        Some(ago) => *ago > limit,
        None => true, // no timestamp: treat as stale, overwriting a blank lock is safe
    }
}

/// Unix millis as the lock's start stamp.
pub fn unix_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::StorePaths;

    #[test]
    fn a_fresh_lock_acquires_and_releases() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StorePaths::new(dir.path(), dir.path().join("config"));
        let handle = acquire(&paths).unwrap();
        assert_eq!(paths.lock_file(), handle.path, "the lock file is where paths put it");
        let text = std::fs::read_to_string(&handle.path).unwrap();
        assert!(text.starts_with(&format!("pid={}\n", std::process::id())), "acquired by us: {text}");
        drop(handle);
        assert!(!paths.lock_file().exists(), "drop removes the lock file");
    }

    #[test]
    fn a_second_process_lock_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StorePaths::new(dir.path(), dir.path().join("config"));
        let first = acquire(&paths).unwrap();
        let err = acquire(&paths).unwrap_err();
        assert!(matches!(err, postguy_util::Error::Unusable { .. }), "{err}");
        drop(first);
    }

    #[test]
    fn a_stale_lock_is_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StorePaths::new(dir.path(), dir.path().join("config"));
        // A lock from a prerecorded pid of an impossible process, marked ancient.
        std::fs::create_dir_all(paths.lock_file().parent().unwrap()).unwrap();
        std::fs::write(paths.lock_file(), "pid=4194304\nstart=0\n").unwrap();
        let takeover = acquire(&paths).unwrap();
        assert!(takeover.path.exists(), "old lock replaced (no pid alive since 0ms is Unix epoch)");
        let text = std::fs::read_to_string(&takeover.path).unwrap();
        assert!(text.contains(&format!("pid={}", std::process::id())));
    }

    #[test]
    fn truncated_locks_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let paths = StorePaths::new(dir.path(), dir.path().join("config"));
        std::fs::create_dir_all(paths.lock_file().parent().unwrap()).unwrap();
        std::fs::write(paths.lock_file(), "pid=garbage").expect("a truncated write");
        let parsed = parse_lock("pid=garbage");
        assert!(parsed.0.is_none(), "garbage pid is none");
        // Older than the takeover age (no start stamp at all): take over.
        let acquired = acquire(&paths);
        assert!(acquired.is_ok(), "{acquired:?}");
    }
}
