//! Collection assets: files stored under `<workspace>/assets/<collection id>/` that scripts
//! (`pg.assets`, `require`, `fs`) and body shortcodes (`{{% file('x.jpg') %}}`) may read.
//!
//! Every path a caller supplies is validated lexically ([`asset_rel`]) and, once it exists,
//! canonicalised and required to stay inside the collection's asset directory, so neither `..`
//! nor a symlink can reach outside it. Nothing here runs on a UI thread unless the caller
//! chooses; each call is plain blocking file I/O.
//!
//! Replaced and deleted files keep the store's usual one-generation `<name>.bak`; those are
//! never listed.

use std::path::{Path, PathBuf};

use postguy_model::Id;
use serde::Serialize;

use crate::Engine;
use crate::console::{ConsoleKind, ConsoleLevel};
use crate::error::{EngineError, Result};

/// Deepest directory nesting walked or accepted.
const MAX_DEPTH: usize = 16;
/// Most files listed or imported at once.
const MAX_FILES: usize = 10_000;
/// Largest single asset.
const MAX_FILE_BYTES: u64 = 100 << 20;
/// Largest total size of one import.
const MAX_IMPORT_BYTES: u64 = 2 << 30;
/// Longest accepted relative path and path component.
const MAX_PATH_BYTES: usize = 1024;
const MAX_COMPONENT_BYTES: usize = 255;

/// One stored asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetInfo {
    /// Collection-relative path with `/` separators.
    pub path: String,
    pub size: u64,
    pub modified_ms: i64,
}

/// What an import stored and what it refused (with the reason).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetImportOutcome {
    pub imported: Vec<AssetInfo>,
    pub skipped: Vec<String>,
}

fn bad(reason: impl Into<String>) -> EngineError {
    EngineError::BadRequest { reason: reason.into() }
}

/// The plain message of an engine error (no "cannot send:" prefix), for script hosts.
pub(crate) fn error_text(err: &EngineError) -> String {
    match err {
        EngineError::BadRequest { reason } => reason.clone(),
        other => other.to_string(),
    }
}

/// Validate a collection-relative asset path and return it normalised (`a/b.jpg`).
pub(crate) fn asset_rel(path: &str) -> Result<String> {
    if path.is_empty() {
        return Err(bad("asset path is empty"));
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(bad(format!("asset path is longer than {MAX_PATH_BYTES} bytes")));
    }
    if path.contains('\\') {
        return Err(bad(format!("asset path `{path}` may not contain `\\`; use `/`")));
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        if part.is_empty() {
            return Err(bad(format!("asset path `{path}` may not be absolute or contain empty segments")));
        }
        if part == "." || part == ".." {
            return Err(bad(format!("asset path `{path}` may not contain `{part}`")));
        }
        if part.starts_with('.') {
            return Err(bad(format!("asset path `{path}` may not contain hidden names like `{part}`")));
        }
        if part.chars().any(char::is_control) {
            return Err(bad(format!("asset path `{path}` may not contain control characters")));
        }
        if part.len() > MAX_COMPONENT_BYTES {
            return Err(bad(format!("asset path `{path}` has a segment longer than {MAX_COMPONENT_BYTES} bytes")));
        }
        parts.push(part);
    }
    if parts.len() > MAX_DEPTH {
        return Err(bad(format!("asset path `{path}` is nested deeper than {MAX_DEPTH} folders")));
    }
    Ok(parts.join("/"))
}

fn visible_name(name: &str) -> bool {
    !name.starts_with('.') && !name.ends_with(".bak")
}

fn modified_ms(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// Regular files under `root` (depth-capped, symlinks and dotfiles skipped) as
/// `(absolute path, relative path)`. `true` means the file cap cut the walk short.
fn walk_files(root: &Path, skip_bak: bool) -> (Vec<(PathBuf, String)>, bool) {
    let mut files = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new(), 0usize)];
    while let Some((dir, prefix, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else { continue };
            if name.starts_with('.') || (skip_bak && name.ends_with(".bak")) {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            let relative = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if depth + 1 < MAX_DEPTH {
                    stack.push((entry.path(), relative, depth + 1));
                }
            } else if kind.is_file() {
                if files.len() >= MAX_FILES {
                    return (files, true);
                }
                files.push((entry.path(), relative));
            }
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    (files, false)
}

impl Engine {
    fn assets_root(&self, collection: &Id) -> Result<PathBuf> {
        self.store_paths().assets_dir(&collection.0).map_err(|err| bad(err.to_string()))
    }

    /// The on-disk path of `path` inside the collection's assets, refusing anything that
    /// resolves (through symlinks) outside them.
    fn asset_path(&self, collection: &Id, path: &str) -> Result<PathBuf> {
        let rel = asset_rel(path)?;
        let root = self.assets_root(collection)?;
        let target = root.join(&rel);
        let Ok(canonical_root) = root.canonicalize() else { return Ok(target) };
        // The nearest existing ancestor decides: a symlinked directory in the middle of the
        // path would otherwise let a not-yet-existing file land outside.
        let mut probe = target.as_path();
        loop {
            if let Ok(resolved) = probe.canonicalize() {
                if !resolved.starts_with(&canonical_root) {
                    return Err(bad(format!("asset `{path}` resolves outside the collection assets")));
                }
                return Ok(target);
            }
            match probe.parent() {
                Some(parent) => probe = parent,
                None => return Ok(target),
            }
        }
    }

    fn warn(&self, message: String) {
        self.console().push(ConsoleLevel::Warning, ConsoleKind::Warning, message);
    }

    /// Every asset of a collection, sorted by path. A collection without assets gives an
    /// empty list.
    pub fn assets(&self, collection: &Id) -> Result<Vec<AssetInfo>> {
        let root = self.assets_root(collection)?;
        let (files, truncated) = walk_files(&root, true);
        if truncated {
            self.warn(format!("collection `{}` has more than {MAX_FILES} assets; only the first {MAX_FILES} are listed", collection.0));
        }
        let mut infos = Vec::with_capacity(files.len());
        for (absolute, relative) in files {
            if let Ok(metadata) = std::fs::symlink_metadata(&absolute) {
                infos.push(AssetInfo { path: relative, size: metadata.len(), modified_ms: modified_ms(&metadata) });
            }
        }
        infos.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(infos)
    }

    /// The names of the direct children of `dir` (`""` is the asset root), files and folders.
    pub fn list_asset_names(&self, collection: &Id, dir: &str) -> Result<Vec<String>> {
        let directory = if dir.is_empty() { self.assets_root(collection)? } else { self.asset_path(collection, dir.trim_end_matches('/'))? };
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && dir.is_empty() => return Ok(Vec::new()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(bad(format!("asset folder `{dir}` does not exist in this collection")));
            }
            Err(err) => return Err(bad(format!("asset folder `{dir}` cannot be listed: {err}"))),
        };
        let mut names = Vec::new();
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else { continue };
            let Ok(kind) = entry.file_type() else { continue };
            if visible_name(&name) && !kind.is_symlink() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    /// The bytes of one asset (at most 100 MiB).
    pub fn read_asset(&self, collection: &Id, path: &str) -> Result<Vec<u8>> {
        let file = self.asset_path(collection, path)?;
        if !file.is_file() {
            return Err(bad(format!("asset `{path}` does not exist in this collection; import it on the collection's Assets tab")));
        }
        let cap = usize::try_from(MAX_FILE_BYTES).unwrap_or(usize::MAX);
        Ok(postguy_util::read_file_capped(&file, cap)?)
    }

    /// Copy files and folders into the collection. A file lands at `into_dir/<name>`, a
    /// folder at `into_dir/<folder>/…` with its structure; the caps skip (with a reason).
    pub fn import_assets(&self, collection: &Id, sources: &[PathBuf], into_dir: &str, replace: bool) -> Result<AssetImportOutcome> {
        let into = if into_dir.is_empty() { String::new() } else { asset_rel(into_dir.trim_end_matches('/'))? };
        let join = |relative: &str| if into.is_empty() { relative.to_string() } else { format!("{into}/{relative}") };
        let mut files: Vec<(PathBuf, String)> = Vec::new();
        let mut skipped = Vec::new();
        for source in sources {
            let metadata = match std::fs::symlink_metadata(source) {
                Ok(metadata) => metadata,
                Err(err) => {
                    skipped.push(format!("{}: {err}", source.display()));
                    continue;
                }
            };
            let Some(name) = source.file_name().and_then(|name| name.to_str()) else {
                skipped.push(format!("{}: the name is not valid UTF-8", source.display()));
                continue;
            };
            if metadata.file_type().is_symlink() {
                skipped.push(format!("{}: symbolic links are not imported", source.display()));
            } else if metadata.is_dir() {
                let (walked, truncated) = walk_files(source, false);
                if truncated {
                    skipped.push(format!("{}: more than {MAX_FILES} files; the rest were not imported", source.display()));
                }
                files.extend(walked.into_iter().map(|(absolute, relative)| (absolute, join(&format!("{name}/{relative}")))));
            } else {
                files.push((source.clone(), join(name)));
            }
        }
        let mut outcome = self.import_asset_files(collection, &files, replace)?;
        skipped.append(&mut outcome.skipped);
        outcome.skipped = skipped;
        Ok(outcome)
    }

    /// The copy routine behind imports: each `(source file, relative destination)` keeps its
    /// relative path (Bruno import uses it for `lib/utils.js`). Failures and cap hits go to
    /// `skipped`; only a missing collection or bad id aborts.
    pub(crate) fn import_asset_files(&self, collection: &Id, files: &[(PathBuf, String)], replace: bool) -> Result<AssetImportOutcome> {
        let stored = self.store_paths().collection_file(&collection.0).map_err(|err| bad(err.to_string()))?;
        if !stored.is_file() {
            return Err(EngineError::NotFound { what: "collection".into(), id: collection.0.clone() });
        }
        let mut outcome = AssetImportOutcome::default();
        let mut total = 0u64;
        for (index, (source, destination)) in files.iter().enumerate() {
            if index >= MAX_FILES {
                outcome.skipped.push(format!("{} files were not imported: at most {MAX_FILES} assets per import", files.len().saturating_sub(index)));
                break;
            }
            let shown = source.display();
            let relative = match asset_rel(destination) {
                Ok(relative) => relative,
                Err(err) => {
                    outcome.skipped.push(format!("{shown}: {}", error_text(&err)));
                    continue;
                }
            };
            let metadata = match std::fs::symlink_metadata(source) {
                Ok(metadata) if metadata.is_file() => metadata,
                Ok(_) => {
                    outcome.skipped.push(format!("{shown}: not a regular file"));
                    continue;
                }
                Err(err) => {
                    outcome.skipped.push(format!("{shown}: {err}"));
                    continue;
                }
            };
            if metadata.len() > MAX_FILE_BYTES {
                outcome.skipped.push(format!("{shown}: larger than the {} MiB asset cap", MAX_FILE_BYTES >> 20));
                continue;
            }
            if total.saturating_add(metadata.len()) > MAX_IMPORT_BYTES {
                outcome.skipped.push(format!("{shown}: the import would exceed {} GiB", MAX_IMPORT_BYTES >> 30));
                continue;
            }
            match self.store_one(collection, source, &relative, replace) {
                Ok(info) => {
                    total = total.saturating_add(info.size);
                    outcome.imported.push(info);
                }
                Err(err) => outcome.skipped.push(format!("{shown}: {}", error_text(&err))),
            }
        }
        Ok(outcome)
    }

    fn store_one(&self, collection: &Id, source: &Path, relative: &str, replace: bool) -> Result<AssetInfo> {
        let cap = usize::try_from(MAX_FILE_BYTES).unwrap_or(usize::MAX);
        let bytes = postguy_util::read_file_capped(source, cap)?;
        let wanted = relative;
        let mut relative = wanted.to_string();
        let mut target = self.asset_path(collection, &relative)?;
        if !replace {
            let mut attempt = 1u32;
            while target.exists() {
                attempt += 1;
                if attempt > 1000 {
                    return Err(bad(format!("asset `{relative}` already exists and no free name was found")));
                }
                relative = numbered(wanted, attempt);
                target = self.asset_path(collection, &relative)?;
            }
        }
        postguy_util::atomic_write(&target, &bytes)?;
        let metadata = std::fs::metadata(&target).map_err(|err| bad(format!("asset `{relative}` cannot be read back: {err}")))?;
        Ok(AssetInfo { path: relative, size: metadata.len(), modified_ms: modified_ms(&metadata) })
    }

    /// Delete one asset (a `.bak` of it stays, like every store write), then any folders that
    /// became empty.
    pub fn delete_asset(&self, collection: &Id, path: &str) -> Result<()> {
        let file = self.asset_path(collection, path)?;
        if !file.is_file() {
            return Err(EngineError::NotFound { what: "asset".into(), id: path.to_string() });
        }
        postguy_util::atomic_remove(&file)?;
        self.prune_empty_dirs(collection, &file)
    }

    /// Rename or move an asset; refuses to overwrite.
    pub fn rename_asset(&self, collection: &Id, from: &str, to: &str) -> Result<AssetInfo> {
        let source = self.asset_path(collection, from)?;
        let relative = asset_rel(to)?;
        let target = self.asset_path(collection, &relative)?;
        if !source.is_file() {
            return Err(EngineError::NotFound { what: "asset".into(), id: from.to_string() });
        }
        if target.exists() {
            return Err(bad(format!("asset `{to}` already exists")));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|err| bad(format!("cannot create the folder for `{to}`: {err}")))?;
        }
        std::fs::rename(&source, &target).map_err(|err| bad(format!("cannot rename `{from}`: {err}")))?;
        self.prune_empty_dirs(collection, &source)?;
        let metadata = std::fs::metadata(&target).map_err(|err| bad(format!("asset `{to}` cannot be read back: {err}")))?;
        Ok(AssetInfo { path: relative, size: metadata.len(), modified_ms: modified_ms(&metadata) })
    }

    /// Remove empty parent directories of `removed`, stopping at the asset root.
    fn prune_empty_dirs(&self, collection: &Id, removed: &Path) -> Result<()> {
        let root = self.assets_root(collection)?;
        let mut current = removed.parent();
        while let Some(dir) = current {
            if dir == root || !dir.starts_with(&root) || std::fs::remove_dir(dir).is_err() {
                break;
            }
            current = dir.parent();
        }
        Ok(())
    }

    /// Copy every asset of `source` into `copy` (duplicate collection). A failure is a console
    /// warning, never an error: the duplicate itself already succeeded. (`duplicate_collection`
    /// calls it; so does the `collection_duplicate` tool, which saves the copy itself.)
    pub fn copy_collection_assets(&self, source: &Id, copy: &Id) {
        let (Ok(from), Ok(to)) = (self.assets_root(source), self.assets_root(copy)) else { return };
        if !from.is_dir() {
            return;
        }
        let (files, truncated) = walk_files(&from, false);
        if truncated {
            self.warn(format!("collection `{}` has more than {MAX_FILES} assets; only the first {MAX_FILES} were copied", source.0));
        }
        for (absolute, relative) in files {
            let destination = to.join(&relative);
            let copied =
                destination.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::copy(&absolute, &destination).map(|_| ()));
            if let Err(err) = copied {
                self.warn(format!("duplicate: asset `{relative}` was not copied: {err}"));
            }
        }
    }
}

/// `dir/name.ext` → `dir/name (n).ext`.
fn numbered(relative: &str, attempt: u32) -> String {
    let (dir, file) = match relative.rsplit_once('/') {
        Some((dir, file)) => (Some(dir), file),
        None => (None, relative),
    };
    let renamed = match file.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({attempt}){}", file.get(..dot).unwrap_or(file), file.get(dot..).unwrap_or_default()),
        _ => format!("{file} ({attempt})"),
    };
    match dir {
        Some(dir) => format!("{dir}/{renamed}"),
        None => renamed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_paths_are_validated() {
        assert_eq!(asset_rel("a/b.jpg").as_deref().ok(), Some("a/b.jpg"));
        for bad_path in ["", "/abs", "../x", "a/../x", "a//b", ".hidden", "a/.env", "a\\b", "a/\u{0}b", "./a"] {
            assert!(asset_rel(bad_path).is_err(), "{bad_path:?} should be refused");
        }
        assert!(asset_rel(&"x".repeat(256)).is_err());
        assert!(asset_rel(&"a/".repeat(17)).is_err());
    }

    #[test]
    fn numbering_goes_before_the_extension() {
        assert_eq!(numbered("a/b.jpg", 2), "a/b (2).jpg");
        assert_eq!(numbered("b", 3), "b (3)");
        assert_eq!(numbered("x.tar.gz", 2), "x.tar (2).gz");
    }

    #[test]
    fn import_asset_files_keeps_relative_destinations() {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = Engine::open(postguy_store::StorePaths::new(dir.path().join("ws"), dir.path().join("cfg"))).expect("engine");
        let collection = engine.create_collection("c").expect("collection").id;
        let source = dir.path().join("utils.js");
        std::fs::write(&source, b"module.exports = 1;").expect("source");
        let outcome = engine
            .import_asset_files(&collection, &[(source.clone(), "lib/utils.js".to_string()), (source, "lib/../x.js".to_string())], false)
            .expect("import");
        let imported: Vec<&str> = outcome.imported.iter().map(|info| info.path.as_str()).collect();
        assert_eq!(imported, ["lib/utils.js"]);
        assert_eq!(outcome.skipped.len(), 1, "{:?}", outcome.skipped);
        assert_eq!(engine.read_asset(&collection, "lib/utils.js").expect("read"), b"module.exports = 1;");
    }
}
