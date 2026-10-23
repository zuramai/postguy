//! Root confinement (AGENTS.md 3): every path a tool touches resolves inside one allowed
//! directory. The refusal is IDENTICAL for existing and missing outside paths, so a confined
//! agent cannot learn what the disk holds. Relative paths resolve inside the root; `..` is
//! resolved by components BEFORE the check; absolute paths may be absolute-inside the root.
//!
//! With `root: None` (no confinement requested) paths resolve against the process CWD; the
//! tools still refuse NUL bytes and a missing final component's parent chain beyond CWD? No —
//! without confinement the caller asked for it; the safety rules about identical refusals
//! apply only when a root is set (the CLI sets one for `run --script --root`).

use std::path::{Component, Path, PathBuf};

use crate::error::{AutomationError, Result};

/// The confinement root: a canonicalized directory every path must stay inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    dir: PathBuf,
}

impl Root {
    /// Canonicalize `dir` (it must exist; `workspace_open` creates it first when the caller
    /// lets it). A root that cannot be canonicalized is a plain error, not a refusal.
    pub fn new(dir: &Path) -> Result<Root> {
        let dir = std::fs::canonicalize(dir)
            .map_err(|err| AutomationError::Failed { message: format!("allowed directory {} cannot be resolved: {err}", dir.display()) })?;
        Ok(Root { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Resolve `given` inside the root. The refusal names `given` exactly as the caller wrote
    /// it and the root; it is identical whether `given` exists or not.
    ///
    /// Absolute paths have two valid spellings on a system with a symlinked prefix (macOS
    /// `/var` → `/private/var`): the lexical check runs first, then — for absolute paths only —
    /// the closest existing ancestor is canonicalized so a path the user reaches through the
    /// symlink still counts as inside. An outside path fails both checks with the same refusal.
    pub fn resolve(&self, given: &str) -> Result<PathBuf> {
        if given.contains('\0') {
            return Err(self.refuse(given));
        }
        let candidate = Path::new(given);
        // Absolute paths are joined as-is (then checked); relative ones hang off the root.
        let joined = if candidate.is_absolute() { candidate.to_path_buf() } else { self.dir.join(candidate) };
        let normalized = normalize(&joined);
        if normalized.starts_with(&self.dir) {
            return Ok(normalized);
        }
        if candidate.is_absolute()
            && let Some(real) = self.canonical_prefix_match(&normalized)
        {
            return Ok(real);
        }
        Err(self.refuse(given))
    }

    /// Walk up to the closest existing ancestor, canonicalize it, and rejoin the missing tail.
    /// Returns the canonical path when it lands inside the root.
    fn canonical_prefix_match(&self, normalized: &Path) -> Option<PathBuf> {
        let mut probe = normalized.to_path_buf();
        let mut suffix: Vec<std::ffi::OsString> = Vec::new();
        loop {
            match std::fs::canonicalize(&probe) {
                Ok(real) => {
                    if !real.starts_with(&self.dir) {
                        return None;
                    }
                    let mut out = real;
                    for part in suffix.iter().rev() {
                        out.push(part);
                    }
                    return Some(out);
                }
                Err(_) => {
                    let (parent, name) = (probe.parent()?, probe.file_name()?);
                    suffix.push(name.to_os_string());
                    probe = parent.to_path_buf();
                }
            }
        }
    }

    /// Resolve a path that must exist, following symlinks so a link cannot escape the root.
    /// When the final component (or part of the chain) does not exist yet, the CLOSEST
    /// EXISTING ancestor is canonicalized and checked, and the missing tail is appended, so a
    /// symlinked directory can never smuggle a not-yet-created file outside.
    pub fn resolve_existing(&self, given: &str) -> Result<PathBuf> {
        let path = self.resolve(given)?;
        let mut probe = path.clone();
        let mut suffix: Vec<std::ffi::OsString> = Vec::new();
        loop {
            match std::fs::canonicalize(&probe) {
                Ok(real) => {
                    if !real.starts_with(&self.dir) {
                        return Err(self.refuse(given));
                    }
                    let mut out = real;
                    for part in suffix.iter().rev() {
                        out.push(part);
                    }
                    return Ok(out);
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    let (Some(parent), Some(name)) = (probe.parent(), probe.file_name()) else {
                        return Ok(path);
                    };
                    suffix.push(name.to_os_string());
                    probe = parent.to_path_buf();
                }
                Err(err) => {
                    return Err(AutomationError::Failed { message: format!("{} cannot be resolved: {err}", path.display()) });
                }
            }
        }
    }

    fn refuse(&self, given: &str) -> AutomationError {
        AutomationError::OutsideRoot { path: given.to_string(), root: self.dir.to_string_lossy().into_owned() }
    }
}

/// Lexical normalization: resolve `.`/`..` by components (no filesystem access, so missing
/// paths normalize the same way existing ones do).
pub fn normalize(path: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => out.push(component),
            },
            other => out.push(other),
        }
    }
    let mut collected = PathBuf::new();
    for component in out {
        collected.push(component.as_os_str());
    }
    collected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_stay_inside() {
        let dir = tempfile::tempdir().unwrap();
        let root = Root::new(dir.path()).unwrap();
        let inside = root.resolve("data/note.json").unwrap();
        assert!(inside.starts_with(root.dir()));
        assert_eq!(inside, root.dir().join("data/note.json"));
    }

    #[test]
    fn dotdot_escapes_refuse_before_touching_the_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let root = Root::new(dir.path()).unwrap();
        let err = root.resolve("../outside.json").unwrap_err();
        assert!(matches!(err, AutomationError::OutsideRoot { .. }), "{err}");
        // Lexical normalization: an interior `..` that stays inside is fine.
        let ok = root.resolve("a/../b.txt").unwrap();
        assert_eq!(ok, root.dir().join("b.txt"));
    }

    #[test]
    fn the_refusal_is_identical_for_existing_and_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = Root::new(dir.path()).unwrap();
        let missing = root.resolve("../ghost.json").unwrap_err().to_string();
        std::fs::write(dir.path().parent().unwrap().join("real-outside.json"), b"x").unwrap();
        let existing = root.resolve("../real-outside.json").unwrap_err().to_string();
        // Both name the path AS GIVEN and the same root: existence never leaks.
        assert_eq!(missing, format!("../ghost.json is outside the allowed directory {}", dir.path().canonicalize().unwrap().display()));
        assert_eq!(existing, format!("../real-outside.json is outside the allowed directory {}", dir.path().canonicalize().unwrap().display()));
        let _ = std::fs::remove_file(dir.path().parent().unwrap().join("real-outside.json"));
    }

    #[test]
    fn absolute_paths_are_checked_and_absolute_inside_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let root = Root::new(dir.path()).unwrap();
        let inside = root.dir().join("nested/file.json");
        let as_str = inside.to_string_lossy().into_owned();
        assert_eq!(root.resolve(&as_str).unwrap(), inside);
        assert!(root.resolve("/etc/hosts").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn symlinks_cannot_escape_when_resolving_existing() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = dir.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let root = Root::new(dir.path()).unwrap();
        let err = root.resolve_existing("escape/secret.txt").unwrap_err();
        assert!(matches!(err, AutomationError::OutsideRoot { .. }), "{err}");
        // A missing path under the symlink ALSO refuses: canonicalize of the parent.
        let err = root.resolve_existing("escape/ghost.txt").unwrap_err();
        assert!(err.to_string().contains("outside the allowed directory"), "{err}");
    }

    #[test]
    fn normalize_handles_deep_dotdot_chains() {
        let path = Path::new("/a/b/../c/./d/../../e");
        assert_eq!(normalize(path), PathBuf::from("/a/e"));
    }
}
