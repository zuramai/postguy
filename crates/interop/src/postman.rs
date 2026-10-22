//! Postman vocabulary import/export: Collection v2.1/v2.0 (JSON), environments and globals.
//!
//! # FIDELITY (AGENTS.md 5)
//!
//! Every key the Postman schemas define, and every unknown key the file carries, survives an
//! import → export round trip:
//!
//! - Known keys map onto `postguy-model` structs (`Collection`, `Item`, `Folder`, `Request`,
//!   `HttpRequest`, `Body`, `AuthConfig`, `KeyValue`, `Variable`, `Example`,
//!   `RequestSettings`).
//! - Unknown keys land in the mapped struct's `#[serde(flatten)] extras` — including leftover
//!   keys the mapper sees while mapping (`description` objects next to a `String` target,
//!   `in`-aliases, url-object parts) so nothing is silently rewritten.
//! - A small set of keys prefixed `postman` is RESERVED for shape information the model cannot
//!   store literally (e.g. `postmanUrlIsString` for a string-form URL, `postmanSchema` for the
//!   v2.0 URL). They are consumed by the exporter and never emitted as user data; the export
//!   therefore reproduces the original Postman shapes.
//!
//! # Decisions
//!
//! - `import_collection` detects the generation from `info.schema`: v2.1 (default), v2.0;
//!   v1 Collection Format and v3 YAML are DETECTED and reported with per-format instructions
//!   as [`PostmanError::UnsupportedFormat`].
//! - URL objects map onto the `HttpRequest.url` STRING (postman `url.raw` is the source of
//!   truth; `{{vars}}` and `:pathVars` preserved verbatim). `url.query[]` becomes the params
//!   two-way view; `url.variable[]` becomes the path-vars view. On export the object form is
//!   REGENERATED from the string + rows (postguy-urlkit splits the string), so ordering and
//!   duplicate rows follow the row lists.
//! - Body modes raw/urlencoded/formdata/file/graphql map one-to-one onto
//!   `postguy_model::Body`; `source`-side only: `options.raw.language` maps to `RawLanguage`
//!   (unknown languages ride along verbatim).
//! - Auth maps per the Postman attribute spellings ([`auth`]); typed kinds the model has map
//!   onto the typed structs, `jwt`/`asap` (Postman-typed but outside the model contract) and
//!   any kind with unmappable values stay in [`AuthConfig::Unknown`] verbatim. Attribute rows
//!   (`{key,value,type}`) collapse to the last non-empty value; non-string values are kept in
//!   the auth struct's `extras`.
//! - Item ids: Postman items rarely carry ids; we generate them ([`postguy_model::new_id`]).
//!   Duplicate `_postman_id` across a multi-file import is the CALLER's name-clash layer:
//!   the duplicate is accepted with a warning, no rewrite here.
//! - `disabled`/`enabled` rows: missing `enabled` defaults to `true` for every row kind;
//!   `disabled: true` → `enabled: false` and back on export.
//! - `protocolProfileBehavior`: known keys map to `RequestSettings` (`followRedirects`,
//!   `followOriginalHttpMethod`, `disableUrlEncoding`, `maxRedirects`, `strictSSL`,
//!   `sslNoVerify`, `timeout`); unknown keys survive per item under the reserved
//!   `postmanProtocolProfileBehavior` extras key. Collection-level `protocolProfileBehavior`
//!   has no model field, so it rides along verbatim and is re-emitted as-is.
//! - gRPC/WebSocket-Schema items (Postman `request.protocol`) are listed in the report as
//!   dropped (best-effort only when RECOGNISED); no fake HTTP request is
//!   invented.
//!
//! All functions are total over `&Value`: hostile input degrades to warnings/`dropped`
//! entries, never panics.

use postguy_model::Collection;
use serde_json::Value;

use crate::error::PostmanError;

mod auth;
mod export;
mod import;
mod report;

pub use report::{ImportReport, Imported, MapOutcome};

/// Sniff an already-parsed JSON value (multi-file list, one entry at a time; the LIST
/// operation) and dispatch to the collection importer or the
/// environment/globals importer.
///
/// # Errors
/// Anything that is not a Postman-shaped object, or a corrupted collection/environment, is
/// [`PostmanError`].
pub fn import_value(v: &Value) -> Result<(Imported, ImportReport), PostmanError> {
    match import::sniff(v) {
        import::Kind::Collection => {
            let (collection, report) = import_collection(v)?;
            Ok((Imported::Collection(Box::new(collection)), report))
        }
        import::Kind::Environment => {
            let (environment, report) = import_environment(v)?;
            Ok((Imported::Environment(environment), report))
        }
        import::Kind::Globals => {
            let (globals, report) = import_environment(v)?;
            Ok((Imported::Globals(globals), report))
        }
    }
}

/// Import one Postman collection document (v2.1 by the URL in `info.schema`, or v2.0; v1/
/// v3 inputs are detected and reported with instructions, never guessed to be v2).
///
/// # Errors
/// [`PostmanError`] for missing `info`/`item`, wrong shapes, and detected v1/v3 documents.
pub fn import_collection(v: &Value) -> Result<(Collection, ImportReport), PostmanError> {
    import::import_collection(v)
}

/// Import a Postman environment or globals document (both are `{name, values[],
/// _postman_variable_scope}` shapes; distinguished by [`import_value`]/[`imported_scope`]).
///
/// Rows with no `enabled` import as enabled; `type: "secret"` sets the secret flag; other
/// `type` values are reported as warnings and kept in row extras.
///
/// # Errors
/// [`PostmanError`] for non-object input and for non-object `values` entries.
pub fn import_environment(v: &Value) -> Result<(postguy_model::Environment, ImportReport), PostmanError> {
    import::import_environment(v)
}

/// Export a collection to a Postman Collection v2.1 JSON document. Secrets
/// handling (stripping) is the caller's secrets-store decision; the model only knows the
/// `secret` flag, which round trips.
#[must_use]
pub fn export_collection(model: &Collection) -> Value {
    export::export_collection(model)
}

/// Export an environment or globals document; `scope` selects `_postman_variable_scope` and
/// the shape (`"environment"` or `"globals"`).
#[must_use]
pub fn export_environment(model: &postguy_model::Environment, scope: &str) -> Value {
    export::export_environment(model, scope)
}

/// The scope sniffed at import time, so callers can export with the matching shape.
#[must_use]
pub fn imported_scope(imported: &Imported) -> &'static str {
    match imported {
        Imported::Collection(_) => "collection",
        Imported::Environment(_) => "environment",
        Imported::Globals(_) => "globals",
    }
}

// Presentation of submodules re-exported at this level:

pub use import::import_path;

#[cfg(test)]
mod tests;
