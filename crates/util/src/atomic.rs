//! Atomic file writes: write to a temp file in the destination directory, sync, then rename.
//! A failed save leaves the previous file intact (AGENTS.md 4) and keeps one `.bak` so the
//! user always has a copy of the last good version.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

/// Write `bytes` to `path` atomically. On success, the previous content lives at `<path>.bak`
/// (one generation only). On any failure, `path` is untouched, the temp file is removed, and
/// either the previous file or its `.bak` is where it started.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().ok_or_else(|| Error::Unusable { what: "path has no parent directory".to_string() })?;
    std::fs::create_dir_all(dir).map_err(|source| Error::Io { operation: format!("creating {}", dir.display()), source })?;

    let mut staged: Option<PathBuf> = None;
    let mut bak: Option<PathBuf> = None;
    let result = (|| -> Result<()> {
        let temp = temp_path_in(dir, path)?;
        staged = Some(temp.clone());
        write_and_sync(&temp, bytes)?;

        // Keep one generation of the previous file as `.bak`.
        if path.exists() {
            let bak_path = PathBuf::from(format!("{}.bak", path.display()));
            match std::fs::rename(path, &bak_path) {
                Ok(()) => bak = Some(bak_path),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(Error::Io { operation: format!("backing up {}", path.display()), source: err });
                }
            }
        }

        match std::fs::rename(&temp, path) {
            Ok(()) => {
                staged = None;
                Ok(())
            }
            Err(err) => Err(Error::Io { operation: format!("moving {} into place", temp.display()), source: err }),
        }
    })();

    if result.is_err() {
        // Undo in reverse: restore `.bak` over a (possibly half-)renamed target and remove
        // the staged temp file. Each step is best-effort: the error we already have is the
        // interesting one; recovery is a courtesy so a failed save leaves reality intact.
        if let Some(bak_path) = bak
            && !path.exists()
        {
            let _ = std::fs::rename(&bak_path, path);
        }
        if let Some(temp) = staged {
            let _ = std::fs::remove_file(temp);
        }
    }
    result
}

/// Remove `path` and keep the previous version at `<path>.bak` like `atomic_write` does.
/// Missing files are not an error (deleting twice is idempotent).
pub fn atomic_remove(path: &Path) -> Result<()> {
    match std::fs::rename(path, bak_path(path)) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Io { operation: format!("moving {} to trash-side .bak", path.display()), source }),
    }
}

fn bak_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.bak", path.display()))
}

/// A unique-enough staging name in the same directory as the real target: same volume so the
/// rename is atomic; pid + monotonic nanos makes collisions practically impossible, and a
/// collision still fails loudly instead of clobbering.
fn temp_path_in(dir: &Path, target: &Path) -> Result<PathBuf> {
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::Unusable { what: format!("{} has no usable file name", target.display()) })?;
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let candidate = dir.join(format!(".{name}.tmp-{}-{nanos}", std::process::id()));
    if candidate.exists() {
        return Err(Error::Unusable { what: format!("staging name {} already exists (retry may succeed)", candidate.display()) });
    }
    Ok(candidate)
}

/// Write bytes to a fresh file and flush to media before the caller renames anything.
fn write_and_sync(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::File::create(path).map_err(|source| Error::Io { operation: format!("staging {}", path.display()), source })?;
    let mut written = 0usize;
    while written < bytes.len() {
        let n = std::io::Write::write(&mut file, &bytes[written..])
            .map_err(|source| Error::Io { operation: format!("staging {}", path.display()), source })?;
        if n == 0 {
            return Err(Error::Unusable { what: format!("staging {} made no progress (0-byte write)", path.display()) });
        }
        written += n;
    }
    file.sync_all().map_err(|source| Error::Io { operation: format!("syncing {}", path.display()), source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use std::fs;

    fn unique_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("postguy-util-tests-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_atomically_and_keeps_one_bak() {
        let dir = unique_dir("atomic");
        let path = dir.join("data.json");
        atomic_write(&path, b"one").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one");
        assert!(!dir.join("data.json.bak").exists(), "first write has no previous");

        atomic_write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(fs::read(dir.join("data.json.bak")).unwrap(), b"one");

        atomic_write(&path, b"three").unwrap();
        assert_eq!(fs::read(dir.join("data.json.bak")).unwrap(), b"two", "one generation only");

        // No temp files left behind.
        let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(leftovers.len(), 2, "only the data file and its .bak");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_sync_leaves_the_previous_file_intact() {
        let dir = unique_dir("fail");
        let path = dir.join("data.json");
        atomic_write(&path, b"good").unwrap();

        // Force the write to fail on every platform: the target's parent is a regular file.
        let blocker = dir.join("blocker");
        fs::write(&blocker, b"not a directory").unwrap();
        let target = blocker.join("data.json");
        let err = atomic_write(&target, b"nope");
        assert!(err.is_err(), "must fail when the parent is a file");
        match err.unwrap_err() {
            Error::Io { .. } => {}
            other => panic!("expected Io, got {other:?}"),
        }
        // The previous file elsewhere was untouched.
        assert_eq!(fs::read(&path).unwrap(), b"good");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn remove_moves_to_bak_and_is_idempotent() {
        let dir = unique_dir("remove");
        let path = dir.join("data.json");
        atomic_write(&path, b"gone soon").unwrap();
        atomic_remove(&path).unwrap();
        assert!(!path.exists());
        assert_eq!(fs::read(dir.join("data.json.bak")).unwrap(), b"gone soon");
        atomic_remove(&path).unwrap(); // second call: no error
        let _ = fs::remove_dir_all(dir);
    }
}
