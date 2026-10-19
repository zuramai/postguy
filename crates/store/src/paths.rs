//! Path resolution for the store: one root = one workspace, one config dir per host user.
//! Everything the store touches goes through these two roots; `--root` confinement is a
//! caller-level rule the store also enforces on its own writes (see
//! `StorePaths::contains`).

use std::path::{Path, PathBuf};

use crate::workspace_file::WorkspaceError;

/// Both roots the store touches. Construct it via [`StorePaths::new`]; the engine passes
/// the OS config dir (via `platform`) and the workspace folder separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePaths {
    workspace: PathBuf,
    config: PathBuf,
}

impl StorePaths {
    /// Both roots raw; canonicalized lazily by the OS calls that use them.
    pub fn new(workspace: impl Into<PathBuf>, config: impl Into<PathBuf>) -> StorePaths {
        StorePaths { workspace: workspace.into(), config: config.into() }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn config(&self) -> &Path {
        &self.config
    }

    /// True when `path` lives inside the workspace root (never a sibling or a parent dir).
    pub fn workspace_contains(&self, path: &Path) -> bool {
        crate::paths::inside(&self.workspace, path)
    }

    /// `<workspace>/collections`
    pub fn collections_dir(&self) -> PathBuf {
        self.workspace.join("collections")
    }

    pub fn collection_file(&self, id: &str) -> Result<PathBuf, WorkspaceError> {
        id_file(&self.collections_dir(), id)
    }

    /// `<workspace>/environments`
    pub fn environments_dir(&self) -> PathBuf {
        self.workspace.join("environments")
    }

    pub fn environment_file(&self, id: &str) -> Result<PathBuf, WorkspaceError> {
        id_file(&self.environments_dir(), id)
    }

    pub fn globals_file(&self) -> PathBuf {
        self.workspace.join("globals.json")
    }

    pub fn workspace_meta_file(&self) -> PathBuf {
        self.workspace.join("postguy.json")
    }

    pub fn protos_dir(&self) -> PathBuf {
        self.workspace.join("protos")
    }

    pub fn files_dir(&self) -> PathBuf {
        self.workspace.join("files")
    }

    pub fn history_file(&self) -> PathBuf {
        self.workspace.join("history.jsonl")
    }

    pub fn trash_dir(&self) -> PathBuf {
        self.workspace.join(".trash")
    }

    /// The private side-car directory that hides secrets, cookies and the cache.
    pub fn dot_dir(&self) -> PathBuf {
        self.workspace.join(".postguy")
    }

    pub fn secrets_file(&self) -> PathBuf {
        self.dot_dir().join("secrets.json")
    }

    pub fn cookies_file(&self) -> PathBuf {
        self.dot_dir().join("cookies.json")
    }

    pub fn responses_dir(&self) -> PathBuf {
        self.dot_dir().join("responses")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.dot_dir().join("lock")
    }

    /// `<config>/settings.json` (PROMPT.md 4.4: user config NOT inside the workspace).
    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    /// `<config>/session.json` (open tabs, drafts, window geometry...).
    pub fn session_file(&self) -> PathBuf {
        self.config.join("session.json")
    }

    /// `<config>/recent.json` for recently opened workspaces.
    pub fn recent_file(&self) -> PathBuf {
        self.config.join("recent.json")
    }
}

/// A file inside `dir` named after an id. Only safe id spellings are accepted: UUID text is
/// `[0-9a-fA-F-]{36}`; anything else is a refusing error so a hostile id can never traverse.
pub fn id_file(dir: &Path, id: &str) -> Result<PathBuf, WorkspaceError> {
    let safe_id = matches!(id.len(), 36) && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') && id.chars().filter(|c| *c == '-').count() == 4;
    if !safe_id {
        return Err(WorkspaceError::BadId { id: id.to_string() });
    }
    Ok(dir.join(format!("{id}.json")))
}

/// `child` resolves inside `root` — by component comparison, without touching the filesystem.
/// Store paths are always built through [`StorePaths`], so the check is lexical and total;
/// hostile `..` chains and prefixes can never fool it.
pub fn inside(root: &Path, child: &Path) -> bool {
    let root = root.components().collect::<std::path::PathBuf>();
    let child = child.components().collect::<std::path::PathBuf>();
    child.starts_with(&root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_files_accept_uuid_shape_only() {
        let dir = PathBuf::from("/tmp/never-here");
        assert!(id_file(&dir, "01234567-89ab-cdef-0123-456789abcdef").is_ok());
        assert!(id_file(&dir, "../escape").is_err(), "an id can never traverse");
        assert!(id_file(&dir, "short").is_err());
        assert!(id_file(&dir, "01234567-89ab-cdef-0123-456789abcdeff").is_err());
    }
}
