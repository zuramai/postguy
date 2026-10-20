//! All-data workspace zip export and reading a zip back through the importers (M4.3).
//!
//! Export walks the workspace itself (collections, environments, globals, protos, files,
//! history/meta) and deliberately skips `.postguy/` (secrets, cookies, response cache, lock) and
//! `.trash/`, so an export never ships credentials by accident. Import reads only `.json`
//! documents and feeds them through [`Engine::import_text`], so name clashes get the same
//! suffix/replace rules as every other import; arbitrary entries are never extracted to disk.
//! Both paths cap entry counts and bytes and reject traversal names.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::Engine;
use crate::error::{EngineError, Result};
use crate::import::{IMPORT_FILE_CAP, ImportOptions, ImportOutcome};

/// Maximum files in one workspace zip.
pub const ZIP_ENTRY_CAP: usize = 50_000;
/// Maximum uncompressed bytes read or written for one zip.
pub const ZIP_TOTAL_CAP: u64 = 1 << 30;
/// Entries read during import (a hostile archive cannot make us loop forever).
const ZIP_IMPORT_ENTRY_CAP: usize = 5_000;

/// One zip entry processed by [`Engine::import_zip`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZipEntryOutcome {
    pub source: String,
    pub outcome: Option<ImportOutcome>,
    pub error: Option<String>,
}

/// The aggregate result of importing one zip.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ZipImportOutcome {
    pub items: Vec<ZipEntryOutcome>,
    pub skipped: Vec<String>,
}

impl Engine {
    /// Write every workspace document (not `.postguy/` or `.trash/`) into a compressed zip.
    /// The archive is written to a sibling temp file and renamed, so a failure leaves the
    /// previous archive intact.
    pub fn export_workspace_zip(&self, out: &Path) -> Result<u64> {
        let root = self.store_paths().workspace().to_path_buf();
        let parent = out.parent().ok_or_else(|| EngineError::Import { message: "the export path has no parent folder".into() })?;
        std::fs::create_dir_all(parent).map_err(|err| EngineError::Import { message: format!("creating {}: {err}", parent.display()) })?;
        let temp =
            parent.join(format!(".{}.partial-{}", out.file_name().and_then(|name| name.to_str()).unwrap_or("workspace.zip"), std::process::id()));
        let file = std::fs::File::create(&temp).map_err(|err| EngineError::Import { message: format!("creating {}: {err}", temp.display()) })?;
        let result = (|| -> Result<u64> {
            let mut writer = ZipWriter::new(file);
            let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            let mut files = Vec::new();
            collect(&root, &root, 0, &mut files)?;
            files.sort();
            let mut total = 0u64;
            for path in files {
                let relative = path.strip_prefix(&root).map_err(|_| EngineError::Import { message: "workspace path escaped its root".into() })?;
                let metadata = std::fs::metadata(&path).map_err(|err| EngineError::Import { message: format!("{}: {err}", path.display()) })?;
                let cap_left = ZIP_TOTAL_CAP.saturating_sub(total);
                if metadata.len() > cap_left || metadata.len() > IMPORT_FILE_CAP as u64 {
                    return Err(EngineError::Import {
                        message: format!("{} is too large for a workspace export (cap {ZIP_TOTAL_CAP} bytes total)", path.display()),
                    });
                }
                let name = relative.to_string_lossy().replace('\\', "/");
                writer.start_file(&name, options).map_err(|err| EngineError::Import { message: format!("adding {name}: {err}") })?;
                let mut source = std::fs::File::open(&path).map_err(|err| EngineError::Import { message: format!("{}: {err}", path.display()) })?;
                let copied =
                    std::io::copy(&mut source, &mut writer).map_err(|err| EngineError::Import { message: format!("adding {name}: {err}") })?;
                total = total.saturating_add(copied);
            }
            writer.finish().map_err(|err| EngineError::Import { message: format!("finishing the zip: {err}") })?;
            Ok(total)
        })();
        match result {
            Ok(total) => {
                #[cfg(windows)]
                if out.exists() {
                    let _ = std::fs::remove_file(out);
                }
                std::fs::rename(&temp, out).map_err(|err| {
                    let _ = std::fs::remove_file(&temp);
                    EngineError::Import { message: format!("replacing {}: {err}", out.display()) }
                })?;
                Ok(total)
            }
            Err(err) => {
                let _ = std::fs::remove_file(&temp);
                Err(err)
            }
        }
    }

    /// Read a zip (all-data export or a folder of Postman JSON files) and import every `.json`
    /// entry through the regular importer. Entries are never written to arbitrary paths.
    pub fn import_zip(&self, path: &Path, options: ImportOptions) -> Result<ZipImportOutcome> {
        let file = std::fs::File::open(path).map_err(|err| EngineError::Import { message: format!("{}: {err}", path.display()) })?;
        let mut archive = ZipArchive::new(file).map_err(|err| EngineError::Import { message: format!("{}: {err}", path.display()) })?;
        let mut result = ZipImportOutcome::default();
        let mut total = 0u64;
        let count = archive.len().min(ZIP_IMPORT_ENTRY_CAP);
        if archive.len() > ZIP_IMPORT_ENTRY_CAP {
            result.skipped.push(format!("zip has {} entries; only the first {ZIP_IMPORT_ENTRY_CAP} were read", archive.len()));
        }
        for index in 0..count {
            let mut entry = match archive.by_index(index) {
                Ok(entry) => entry,
                Err(err) => {
                    result.skipped.push(format!("entry {index}: {err}"));
                    continue;
                }
            };
            let name = match entry.name() {
                Ok(name) => name.to_string(),
                Err(err) => {
                    result.skipped.push(format!("entry {index}: {err}"));
                    continue;
                }
            };
            if entry.enclosed_name().is_none() {
                result.skipped.push(format!("{name}: path traversal refused"));
                continue;
            }
            let is_json = name.to_ascii_lowercase().ends_with(".json");
            if !is_json || is_workspace_metadata(&name) {
                result.skipped.push(name);
                continue;
            }
            if entry.size() > IMPORT_FILE_CAP as u64 || total.saturating_add(entry.size()) > ZIP_TOTAL_CAP {
                result.items.push(ZipEntryOutcome { source: name, outcome: None, error: Some("entry exceeds the import size cap".into()) });
                continue;
            }
            let mut bytes = Vec::with_capacity(entry.size().min(IMPORT_FILE_CAP as u64) as usize);
            if let Err(err) = entry.by_ref().take(IMPORT_FILE_CAP as u64).read_to_end(&mut bytes) {
                result.items.push(ZipEntryOutcome { source: name, outcome: None, error: Some(err.to_string()) });
                continue;
            }
            total = total.saturating_add(bytes.len() as u64);
            match String::from_utf8(bytes) {
                Ok(text) => match self.import_text(&text, options) {
                    Ok(outcome) => result.items.push(ZipEntryOutcome { source: name, outcome: Some(outcome), error: None }),
                    Err(err) => result.items.push(ZipEntryOutcome { source: name, outcome: None, error: Some(err.to_string()) }),
                },
                Err(_) => result.items.push(ZipEntryOutcome { source: name, outcome: None, error: Some("not UTF-8 JSON text".into()) }),
            }
        }
        Ok(result)
    }
}

/// Metadata/docs inside an all-data zip that are not importable Postman documents.
fn is_workspace_metadata(name: &str) -> bool {
    let file = name.rsplit('/').next().unwrap_or(name);
    file.eq_ignore_ascii_case("postguy.json")
        || file.eq_ignore_ascii_case("meta.json")
        || file.eq_ignore_ascii_case("history.jsonl")
        || file.eq_ignore_ascii_case("session.json")
        || file.eq_ignore_ascii_case("postguy.lock")
}

fn collect(root: &Path, dir: &Path, depth: usize, files: &mut Vec<PathBuf>) -> Result<()> {
    if depth > 32 {
        return Err(EngineError::Import { message: format!("{}: folder depth cap reached", dir.display()) });
    }
    if files.len() >= ZIP_ENTRY_CAP {
        return Err(EngineError::Import { message: format!("workspace has more than {ZIP_ENTRY_CAP} files") });
    }
    let entries = std::fs::read_dir(dir).map_err(|err| EngineError::Import { message: format!("{}: {err}", dir.display()) })?;
    for entry in entries {
        let entry = entry.map_err(|err| EngineError::Import { message: format!("{}: {err}", dir.display()) })?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let metadata = std::fs::symlink_metadata(&path).map_err(|err| EngineError::Import { message: format!("{}: {err}", path.display()) })?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if depth == 0 && (name == ".postguy" || name == ".trash") {
                continue;
            }
            collect(root, &path, depth + 1, files)?;
        } else if metadata.is_file() {
            if name.ends_with(".bak") {
                continue;
            }
            let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
            if relative.starts_with(".postguy") || relative.starts_with(".trash") {
                continue;
            }
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_names_are_skipped() {
        assert!(is_workspace_metadata("postguy.json"));
        assert!(is_workspace_metadata("nested/session.json"));
        assert!(is_workspace_metadata("history.jsonl"));
        assert!(!is_workspace_metadata("collections/c1.json"));
    }

    #[test]
    fn collect_skips_dot_state_and_backups() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("collections")).expect("collections");
        std::fs::create_dir_all(root.join(".postguy")).expect("state");
        std::fs::write(root.join("collections/c1.json"), "{}").expect("collection");
        std::fs::write(root.join("collections/c1.json.bak"), "{}").expect("backup");
        std::fs::write(root.join(".postguy/secrets.json"), "{}").expect("secrets");
        let mut files = Vec::new();
        collect(root, root, 0, &mut files).expect("collect");
        let names: Vec<String> = files.iter().map(|path| path.strip_prefix(root).expect("rel").to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["collections/c1.json"]);
    }
}
