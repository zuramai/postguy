//! Import entry points shared by the UI and the automation tools: auto-detect Postman
//! collection/environment/globals JSON, OpenAPI/Swagger JSON or YAML, or cURL text, keep the import report, and never
//! overwrite silently. Name clashes get a numeric suffix and a fresh id unless the caller
//! explicitly asks to replace a same-id collection/environment.
//!
//! Parsing is bounded by the interop/util caps; file reads are capped at
//! [`IMPORT_FILE_CAP`]. The workspace writes are atomic (store), so a failed multi-file import
//! leaves the files already imported in place and reports the failures; nothing half-written.

use std::path::{Path, PathBuf};

use postguy_interop::{FolderStrategy, ImportReport, Imported, ParsedCurl};
use postguy_model::{Collection, Environment, Id, Item, Request, RequestKind};
use postguy_util::json::Caps;
use serde::Serialize;

use crate::Engine;
use crate::error::{EngineError, Result};

/// Maximum bytes read for one import file.
pub const IMPORT_FILE_CAP: usize = 100 << 20;
/// Maximum files walked for one folder import; beyond this the report names the cap.
pub const IMPORT_FILE_COUNT_CAP: usize = 2_000;
/// Maximum folder recursion depth.
const MAX_WALK_DEPTH: usize = 32;

/// What the caller may choose about a clash.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOptions {
    /// Only affects a collection/environment whose imported id already exists: overwrite it
    /// instead of assigning a fresh id. Name clashes between different ids still get a suffix.
    pub replace: bool,
    /// How an OpenAPI/Swagger document's operations become folders.
    pub folders: FolderStrategy,
}

/// The document family that was imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportKind {
    Collection,
    Environment,
    Globals,
}

/// One completed import.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub kind: ImportKind,
    /// Collection/environment id; None for globals (a single file).
    pub id: Option<String>,
    pub name: String,
    pub report: ImportReport,
}

/// Text auto-detection: JSON documents from an opening `{`/`[`, everything else as cURL. A
/// URL is refused with an actionable message instead of fetching anything implicitly.
pub fn detect(text: &str) -> Detected {
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if trimmed.is_empty() {
        return Detected::Empty;
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Detected::Json;
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Detected::Url;
    }
    if trimmed.starts_with("%YAML") || first_line_is_yaml_mapping(trimmed) {
        return Detected::Yaml;
    }
    Detected::Curl
}

/// Whether the first content line (after comments and `---`) is a `key: value` mapping entry
/// and not a cURL command (a cURL line starts with `curl`, a YAML key never has spaces).
fn first_line_is_yaml_mapping(text: &str) -> bool {
    let Some(line) = text.lines().map(str::trim).find(|line| !line.is_empty() && !line.starts_with('#') && *line != "---") else { return false };
    if line == "curl" || line.starts_with("curl ") || line.starts_with("curl\t") {
        return false;
    }
    let Some((key, rest)) = line.split_once(':') else { return false };
    !key.is_empty()
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && key.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && (rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detected {
    Json,
    Curl,
    Url,
    Yaml,
    Empty,
}

impl Engine {
    /// Import one text document (Postman or OpenAPI/Swagger JSON, OpenAPI/Swagger YAML or cURL),
    /// applying `options` to clashes. An internal panic becomes an error, never a crash.
    pub fn import_text(&self, text: &str, options: ImportOptions) -> Result<ImportOutcome> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.import_text_inner(text, options))) {
            Ok(result) => result,
            Err(payload) => {
                let message = payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_string());
                Err(EngineError::Import { message: format!("the importer failed unexpectedly: {message}") })
            }
        }
    }

    fn import_text_inner(&self, text: &str, options: ImportOptions) -> Result<ImportOutcome> {
        match detect(text) {
            Detected::Empty => {
                Err(EngineError::Import { message: "nothing to import: paste a collection, environment, globals or cURL command".into() })
            }
            Detected::Url => {
                Err(EngineError::Import { message: "URL imports are not fetched automatically; download the file and drop it here".into() })
            }
            Detected::Yaml => {
                let value = postguy_interop::parse_yaml(text).map_err(|err| EngineError::Import { message: err.to_string() })?;
                if !postguy_interop::is_openapi(&value) {
                    return Err(EngineError::Import { message: "this YAML is not an OpenAPI 3 or Swagger 2 document".into() });
                }
                self.import_openapi_value(&value, options)
            }
            Detected::Json => self.import_json(text, options),
            Detected::Curl => self.import_curl(text, options),
        }
    }

    /// Read a capped UTF-8 file and import it. Directory walking is the caller's job
    /// ([`walk_import_paths`]) so the UI can report each file separately.
    pub fn import_file(&self, path: &Path, options: ImportOptions) -> Result<ImportOutcome> {
        let bytes = postguy_util::read_file_capped(path, IMPORT_FILE_CAP)
            .map_err(|err| EngineError::Import { message: format!("{}: {err}", path.display()) })?;
        let text = String::from_utf8(bytes)
            .map_err(|_| EngineError::Import { message: format!("{} is not UTF-8 text; imports accept JSON and cURL text only", path.display()) })?;
        self.import_text(&text, options)
    }

    fn import_json(&self, text: &str, options: ImportOptions) -> Result<ImportOutcome> {
        let parsed =
            postguy_util::json::parse_json_str(text, &Caps::default()).map_err(|err| EngineError::Import { message: format!("JSON: {err}") })?;
        if postguy_interop::is_openapi(&parsed) {
            return self.import_openapi_value(&parsed, options);
        }
        let (imported, report) = postguy_interop::import_value(&parsed).map_err(|err| EngineError::Import { message: err.to_string() })?;
        match imported {
            Imported::Collection(collection) => self.save_imported_collection(*collection, report, options),
            Imported::Environment(environment) => self.save_imported_environment(environment, report, options),
            Imported::Globals(environment) => self.save_imported_globals(environment, report, options),
        }
    }

    fn import_openapi_value(&self, value: &serde_json::Value, options: ImportOptions) -> Result<ImportOutcome> {
        let (collection, report) =
            postguy_interop::import_openapi(value, options.folders).map_err(|err| EngineError::Import { message: err.to_string() })?;
        self.save_imported_collection(collection, report, options)
    }

    fn import_curl(&self, text: &str, _options: ImportOptions) -> Result<ImportOutcome> {
        let commands = postguy_interop::parse_curl(text).map_err(|err| EngineError::Import { message: err.to_string() })?;
        if commands.is_empty() {
            return Err(EngineError::Import { message: "no cURL command found".into() });
        }
        let mut collection =
            Collection { id: Id::generate(), name: unique_name("Imported cURL", |name| self.collection_name_taken(name)), ..Collection::default() };
        let mut report = ImportReport { collections: 1, ..ImportReport::default() };
        for (index, ParsedCurl { request, warnings, .. }) in commands.into_iter().enumerate() {
            let name = request_name(&request, index);
            report.requests += 1;
            for warning in warnings {
                report.warnings.push(format!("{name}: {warning}"));
            }
            collection.items.push(Item::Request(Box::new(Request {
                id: Id::generate(),
                name,
                kind: RequestKind::Http(request),
                ..Request::default()
            })));
        }
        self.workspace().save_collection(&collection)?;
        Ok(ImportOutcome { kind: ImportKind::Collection, id: Some(collection.id.0), name: collection.name, report })
    }

    fn save_imported_collection(&self, mut collection: Collection, mut report: ImportReport, options: ImportOptions) -> Result<ImportOutcome> {
        let imported_name = if collection.name.trim().is_empty() { "Imported collection".to_string() } else { collection.name.trim().to_string() };
        collection.name = imported_name.clone();
        let old_id = collection.id.0.clone();
        let id_exists = self.collection_id_exists(&collection.id);
        if id_exists && !options.replace {
            collection.id = Id::generate();
            report.warnings.push(format!("collection id {old_id} already exists; imported with a fresh id"));
        }
        if self.collection_name_taken_by_other(&collection.name, &collection.id) {
            collection.name = unique_name(&imported_name, |name| self.collection_name_taken(name));
            report.warnings.push(format!("name clash: `{imported_name}` imported as `{}`", collection.name));
        }
        self.workspace().save_collection(&collection)?;
        Ok(ImportOutcome { kind: ImportKind::Collection, id: Some(collection.id.0), name: collection.name, report })
    }

    fn save_imported_environment(&self, mut environment: Environment, mut report: ImportReport, options: ImportOptions) -> Result<ImportOutcome> {
        let imported_name = if environment.name.trim().is_empty() { "Imported environment".to_string() } else { environment.name.trim().to_string() };
        environment.name = imported_name.clone();
        let mut id_exists = !matches!(self.workspace().load_environment(&environment.id), Ok(postguy_store::LoadOutcome::Fresh(_)) | Err(_));
        if options.replace && !id_exists {
            // Environments have no imported id in the Postman shape; replacement matches the
            // stable user-visible name instead.
            if let Ok(environments) = self.environments()
                && let Some(existing) = environments.iter().find(|existing| existing.name.eq_ignore_ascii_case(&environment.name))
            {
                environment.id = existing.id.clone();
                id_exists = true;
            }
        }
        if id_exists && !options.replace {
            let old_id = environment.id.0.clone();
            environment.id = Id::generate();
            report.warnings.push(format!("environment id {old_id} already exists; imported with a fresh id"));
        }
        if self.environment_name_taken_by_other(&environment.name, &environment.id) {
            environment.name = unique_name(&imported_name, |name| self.environment_name_taken(name));
            report.warnings.push(format!("name clash: `{imported_name}` imported as `{}`", environment.name));
        }
        self.workspace().save_environment(&environment)?;
        Ok(ImportOutcome { kind: ImportKind::Environment, id: Some(environment.id.0), name: environment.name, report })
    }

    fn save_imported_globals(&self, environment: Environment, mut report: ImportReport, options: ImportOptions) -> Result<ImportOutcome> {
        let existing = self.workspace().load_globals().map(|outcome| outcome.into_value()).unwrap_or_default();
        if !options.replace && !existing.variables.is_empty() {
            return Err(EngineError::Import { message: "globals already exist; tick Replace existing to import over them".into() });
        }
        if options.replace && !existing.variables.is_empty() {
            report.warnings.push("replaced the existing globals".into());
        }
        self.workspace().save_globals(&environment)?;
        Ok(ImportOutcome { kind: ImportKind::Globals, id: None, name: environment.name, report })
    }

    fn collection_id_exists(&self, id: &Id) -> bool {
        !matches!(self.workspace().load_collection(id), Ok(postguy_store::LoadOutcome::Fresh(_)) | Err(_))
    }

    fn collection_name_taken(&self, name: &str) -> bool {
        self.collections().is_ok_and(|collections| collections.iter().any(|collection| collection.name.eq_ignore_ascii_case(name)))
    }

    fn environment_name_taken(&self, name: &str) -> bool {
        self.environments().is_ok_and(|environments| environments.iter().any(|environment| environment.name.eq_ignore_ascii_case(name)))
    }

    fn collection_name_taken_by_other(&self, name: &str, own: &Id) -> bool {
        self.collections()
            .is_ok_and(|collections| collections.iter().any(|collection| collection.id != *own && collection.name.eq_ignore_ascii_case(name)))
    }

    fn environment_name_taken_by_other(&self, name: &str, own: &Id) -> bool {
        self.environments()
            .is_ok_and(|environments| environments.iter().any(|environment| environment.id != *own && environment.name.eq_ignore_ascii_case(name)))
    }
}

fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    for suffix in 2..=10_000u32 {
        let candidate = format!("{base} ({suffix})");
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{base} ({})", Id::generate())
}

fn request_name(request: &postguy_model::HttpRequest, index: usize) -> String {
    let url = request.url.split(['?', '#']).next().unwrap_or_default();
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let path = without_scheme.split_once('/').map(|(_, path)| path).unwrap_or_default();
    let tail = path.rsplit('/').find(|part| !part.is_empty());
    match tail {
        Some(tail) => format!("{} {}", request.method.short_label(), tail),
        None => format!("cURL request {}", index + 1),
    }
}

/// Walk user-chosen import paths: files stay files, directories are walked (depth-capped,
/// symlinks skipped) and only regular files are returned. No parsing here; the caller reads
/// each file with [`Engine::import_file`] and reports it separately.
pub fn walk_import_paths(paths: impl IntoIterator<Item = PathBuf>) -> (Vec<PathBuf>, Vec<String>) {
    let mut files = Vec::new();
    let mut problems = Vec::new();
    for path in paths {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => problems.push(format!("{}: symlink skipped", path.display())),
            Ok(metadata) if metadata.is_file() => {
                if files.len() < IMPORT_FILE_COUNT_CAP {
                    files.push(path);
                } else {
                    problems.push(format!("import file cap ({IMPORT_FILE_COUNT_CAP}) reached; the rest were skipped"));
                    break;
                }
            }
            Ok(metadata) if metadata.is_dir() => walk_dir(&path, 0, &mut files, &mut problems),
            Ok(_) => problems.push(format!("{}: not a regular file or folder", path.display())),
            Err(err) => problems.push(format!("{}: {err}", path.display())),
        }
    }
    (files, problems)
}

fn walk_dir(dir: &Path, depth: usize, files: &mut Vec<PathBuf>, problems: &mut Vec<String>) {
    if depth > MAX_WALK_DEPTH {
        problems.push(format!("{}: folder depth cap reached", dir.display()));
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            problems.push(format!("{}: {err}", dir.display()));
            return;
        }
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => paths.push(entry.path()),
            Err(err) => problems.push(format!("{}: {err}", dir.display())),
        }
    }
    paths.sort();
    for path in paths {
        if files.len() >= IMPORT_FILE_COUNT_CAP {
            problems.push(format!("import file cap ({IMPORT_FILE_COUNT_CAP}) reached; the rest were skipped"));
            return;
        }
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {}
            Ok(metadata) if metadata.is_file() => files.push(path),
            Ok(metadata) if metadata.is_dir() => walk_dir(&path, depth + 1, files, problems),
            Ok(_) => {}
            Err(err) => problems.push(format!("{}: {err}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_prefers_json_then_curl_and_refuses_urls() {
        assert_eq!(detect("  {\"info\":{}}"), Detected::Json);
        assert_eq!(detect("[1]"), Detected::Json);
        assert_eq!(detect("curl https://example.test"), Detected::Curl);
        assert_eq!(detect("https://example.test/file.json"), Detected::Url);
        assert_eq!(detect("openapi: 3.0.0\ninfo: {}"), Detected::Yaml);
        assert_eq!(detect("   "), Detected::Empty);
    }

    #[test]
    fn suffixes_are_numeric_and_bounded() {
        assert_eq!(unique_name("Import", |_| false), "Import");
        assert_eq!(unique_name("Import", |name| name == "Import"), "Import (2)");
        assert_eq!(unique_name("Import", |name| name == "Import" || name == "Import (2)"), "Import (3)");
    }

    #[test]
    fn curl_request_names_use_the_last_path_segment() {
        let request = postguy_model::HttpRequest {
            method: postguy_model::HttpMethod::Get,
            url: "https://example.test/v1/payments?limit=2".into(),
            ..Default::default()
        };
        assert_eq!(request_name(&request, 0), "GET payments");
        let request = postguy_model::HttpRequest { url: "https://example.test/".into(), ..Default::default() };
        assert_eq!(request_name(&request, 2), "cURL request 3");
    }
}
