//! Trash (`.trash/`, PROMPT.md 5.5): deleted items land here with a metadata sidecar
//! (`deletedAt`, the original path and the content snapshot). Restore puts the file back
//! (`" restored"` suffix on a clash); purge empties expired entries (30-day default).

use std::path::{Path, PathBuf};

use postguy_util::{Result, atomic_write};
use serde::{Deserialize, Serialize};

/// The default retention window for trashed items (PROMPT.md 5.5: 30 days).
pub const DEFAULT_RETENTION: u64 = 30 * 24 * 3600;

#[derive(Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    /// Where the file lived before deletion (workspace-relative, UTF-8 safe).
    pub original: String,
    pub kind: String,
    /// Unix millis of deletion.
    pub deleted_at_ms: i64,
    /// The file content snapshot path is the entry file itself; the sidecar is adjacent.
    pub size_bytes: Option<u64>,
}

/// Move `file` into the trash: `.<name>.<collision-index>.trash` content plus a
/// `<same>.meta.json` sidecar. The caller deletes nothing themselves.
pub fn move_to_trash(paths: &crate::paths::StorePaths, file: &Path, kind: &str) -> Result<Entry> {
    let trash_dir = paths.trash_dir();
    std::fs::create_dir_all(&trash_dir)
        .map_err(|source| postguy_util::Error::Io { operation: format!("creating {}", trash_dir.display()), source })?;
    let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "item".to_string());
    let stem = name.split('.').next().unwrap_or(&name);
    let id = nanoid_like();
    let mut content = trash_dir.join(id.clone());
    let mut attempt = 1;
    while content.exists() {
        content = trash_dir.join(format!("{id}-{attempt}.trash"));
        attempt += 1;
        if attempt > 99 {
            let _ = stem;
            content = trash_dir.join(format!("{stem}-{id}.trash"));
            break;
        }
    }
    std::fs::rename(file, &content)
        .map_err(|source| postguy_util::Error::Io { operation: format!("moving {} into the trash", file.display()), source })?;
    let size = std::fs::metadata(&content).ok().map(|m| m.len());
    let entry = Entry {
        id: id.clone(),
        original: file.to_string_lossy().into_owned(),
        kind: kind.to_string(),
        deleted_at_ms: crate::lock::unix_ms(),
        size_bytes: size,
    };
    let bytes = serde_json::to_vec_pretty(&entry)
        .map_err(|err| postguy_util::Error::JsonSyntax { what: "writing trash metadata".to_string(), source: err })?;
    atomic_write(&meta_path(&trash_dir, &id), &bytes)?;
    Ok(entry)
}

fn meta_path(trash_dir: &Path, id: &str) -> PathBuf {
    trash_dir.join(format!("{id}.meta.json"))
}

fn nanoid_like() -> String {
    format!("t{}-{}", crate::lock::unix_ms(), std::process::id())
}

/// List the trash: one entry per CONTENT file that has (or once had) a sidecar; entries
/// whose sidecar is gone still list (the file is restorable if the original path is known).
pub fn entries(paths: &crate::paths::StorePaths) -> Result<Vec<Entry>> {
    let trash_dir = paths.trash_dir();
    let read = match std::fs::read_dir(&trash_dir) {
        Ok(read) => read,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(postguy_util::Error::Io { operation: format!("reading {}", trash_dir.display()), source }),
    };
    let mut meta_ids: Vec<(String, Option<Entry>)> = Vec::new();
    for entry in read {
        let entry = entry.map_err(|source| postguy_util::Error::Io { operation: "reading trash entries".to_string(), source })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(id) = name.strip_suffix(".meta.json") {
            let sidecar = trash_dir.join(&name);
            let parsed = serde_json::from_slice::<Entry>(&std::fs::read(&sidecar).unwrap_or_default()).ok();
            meta_ids.push((id.to_string(), parsed));
        }
    }
    Ok(meta_ids.into_iter().filter_map(|(_, entry)| entry).collect())
}

/// Restore one entry to its original path; a clash gets `" restored"` before the extension.
pub fn restore(paths: &crate::paths::StorePaths, entry_id: &str) -> Result<Option<PathBuf>> {
    let trash_dir = paths.trash_dir();
    let meta_file = meta_path(&trash_dir, entry_id);
    let Ok(bytes) = std::fs::read(&meta_file) else { return Ok(None) };
    let entry: Entry = serde_json::from_slice(&bytes)
        .map_err(|err| postguy_util::Error::JsonSyntax { what: format!("parsing trash entry {entry_id}"), source: err })?;
    let original_candidates = [PathBuf::from(&entry.original), {
        let mut path = PathBuf::from(&entry.original);
        let stem = path.file_stem().map(|s| s.to_owned()).map(|s| s.to_string_lossy().into_owned());
        let ext = path.extension().map(|s| s.to_string_lossy().into_owned());
        match (stem, ext) {
            (Some(stem), Some(ext)) => path.set_file_name(format!("{stem} restored.{ext}")),
            (Some(stem), None) => path.set_file_name(format!("{stem} restored")),
            _ => {}
        }
        path
    }];
    let mut settled: Option<PathBuf> = None;
    for candidate in original_candidates.iter() {
        if !candidate.exists() {
            settled = Some(candidate.clone());
            break;
        }
    }
    let Some(target) = settled else { return Ok(None) };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|source| postguy_util::Error::Io { operation: format!("creating {}", parent.display()), source })?;
    }
    std::fs::rename(trash_dir.join(entry_id), &target)
        .map_err(|source| postguy_util::Error::Io { operation: format!("restoring trash entry {entry_id}"), source })?;
    let _ = std::fs::remove_file(&meta_file);
    Ok(Some(target))
}

/// Permanently remove entries older than `retention_secs` (or ALL when a stronger call).
pub fn purge_expired(paths: &crate::paths::StorePaths, retention_secs: u64) -> Result<usize> {
    purge(paths, retention_secs)
}

fn purge(paths: &crate::paths::StorePaths, retention_secs: u64) -> Result<usize> {
    let now = crate::lock::unix_ms();
    let mut purged = 0;
    for entry in entries(paths)? {
        let age = now - entry.deleted_at_ms;
        if age > (retention_secs as i64) * 1000 {
            let trash_dir = paths.trash_dir();
            let _ = std::fs::remove_file(trash_dir.join(&entry.id));
            let _ = std::fs::remove_file(meta_path(&trash_dir, &entry.id));
            purged += 1;
        }
    }
    Ok(purged)
}
