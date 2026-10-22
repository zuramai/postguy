//! Import: Postman collection/environment/globals JSON values → `postguy-model`.
//!
//! Mapping decisions ARE the FIDELITY contract: known keys map onto model fields, unknown
//! keys — and keys whose shape the model cannot hold (an object-form `description` next to a
//! `String` field, a string-form URL, unknown language tags) — ride the `extras` channel of
//! the mapped struct under the reserved `postman*` names ([`reserved`]); the exporter
//! consumes those reserved entries and re-emits the original Postman shape.

use std::path::Path;

use serde_json::{Map, Value};

use postguy_model::{
    AuthConfig, Body, Collection, Environment, Example, ExampleBody, Extras, Folder, HttpRequest, HttpVersion, Item, KeyValue, KeyValueKind,
    RawLanguage, RequestKind, RequestSettings, Script, Scripts, Variable,
};

use postguy_util::json::Caps;

use super::auth::postman_to_auth;
use crate::error::PostmanError;

/// File size cap for path imports.
pub const SIZE_CAP: usize = 100 * 1024 * 1024;
/// JSON nesting cap.
pub const DEPTH_CAP: usize = 256;

/// Reserved extras keys, consumed by the exporter and never emitted as user data.
pub(crate) mod reserved {
    /// The collection's v2.1/v2.0 schema URL, as written in `info.schema`.
    pub const SCHEMA: &str = "postmanSchema";
    /// `info._postman_id` was present (controls whether the id is re-emitted).
    pub const ID_PROVIDED: &str = "postmanIdProvided";
    /// Unknown `info` keys, spread back on export.
    pub const INFO: &str = "postmanInfo";
    /// Collection-level `protocolProfileBehavior`, verbatim (no model field for it).
    pub const COLLECTION_PPB: &str = "postmanProtocolProfileBehavior";
    /// Unknown per-item ppb keys (known keys went to settings); re-spread on export.
    pub const ITEM_PPB: &str = "postmanProtocolProfileBehavior";
    /// A `description` that was an OBJECT at import; re-emitted as that object.
    pub const DESCRIPTION: &str = "postmanDescription";
    /// The URL arrived as a plain string; export emits a string again.
    pub const URL_IS_STRING: &str = "postmanUrlIsString";
    /// Unknown keys of the URL OBJECT besides the mapped ones; re-spread on export.
    pub const URL_EXTRA: &str = "postmanUrl";
    /// The URL was absent entirely; export omits the `url` key.
    pub const URL_ABSENT: &str = "postmanUrlAbsent";
    /// An unmappable HTTP verb, verbatim (e.g. `COPY`); export spells it back.
    pub const METHOD: &str = "postmanMethod";
    /// The GraphQL variables were an OBJECT at import; re-emitted as an object.
    pub const VARIABLES_WAS_OBJECT: &str = "postmanVariablesWasObject";
    /// `script.exec` was exactly one empty string at import; re-emitted as `[""]`.
    pub const EXEC_EMPTY: &str = "postmanExecEmpty";
    /// Unknown `request.options` keys; re-spread on export.
    pub const OPTIONS_EXTRA: &str = "postmanOptions";
    /// Unknown body-level `options` keys; re-spread on export.
    pub const BODY_OPTIONS: &str = "postmanBodyOptions";
    /// Unknown keys of a urlencoded/formdata body object; re-spread on export.
    pub const BODY_EXTRA: &str = "postmanBody";
    /// A description that lives INSIDE the request object (not the item level). The
    /// boolean form (`postmanRequestDescriptionLevel`) marks the level for the export.
    pub const REQUEST_DESCRIPTION: &str = "postmanRequestDescription";
    pub const REQUEST_DESCRIPTION_LEVEL: &str = "postmanRequestDescriptionLevel";
    /// The auth block arrived at the ITEM level (beside `request`).
    pub const AUTH_AT_ITEM_LEVEL: &str = "postmanAuthAtItemLevel";
    /// The per-item ppb arrived INSIDE the request object.
    pub const PPB_AT_REQUEST_LEVEL: &str = "postmanPpbAtRequestLevel";
    /// The request was a bare URL string (the collection format shortcut).
    pub const REQUEST_WAS_STRING: &str = "postmanRequestWasString";
    /// The collection description arrived at the document ROOT (v2.0 style), not in `info`.
    pub const DESCRIPTION_AT_ROOT: &str = "postmanDescriptionAtRoot";
    /// Which keys the URL OBJECT explicitly carried (comma list); the export emits exactly
    /// these (regenerated), so a file that omitted `protocol`/`port` stays omitted.
    pub const URL_KEYS: &str = "postmanUrlKeys";
}

/// What the shape sniff decided about one JSON document.
pub(crate) enum Kind {
    /// `info` + `item[]` present.
    Collection,
    /// `values[]` with scope `environment`, or unmarked.
    Environment,
    /// Scope explicitly `globals`.
    Globals,
}

/// Sniff one JSON document's shape; unknown shapes fall through to the environment branch
/// and the dispatcher reports them precisely when the import fails.
pub(crate) fn sniff(v: &Value) -> Kind {
    let Some(object) = v.as_object() else { return Kind::Environment };
    if object.contains_key("info") && object.contains_key("item") {
        return Kind::Collection;
    }
    if object.get("_postman_variable_scope").and_then(Value::as_str).is_some_and(|s| s.eq_ignore_ascii_case("globals")) {
        return Kind::Globals;
    }
    Kind::Environment
}

/// Import a file from disk: capped read, capped JSON parse, shape sniff, dispatch.
///
/// # Errors
/// I/O and size errors ([`PostmanError::File`]); JSON problems
/// ([`PostmanError::InvalidJson`], including the detected-v3-YAML report) and
/// Postman-shape errors from the import family.
pub fn import_path(path: &Path) -> Result<super::report::Imported, PostmanError> {
    let operation = format!("importing {}", path.display());
    let bytes = postguy_util::read_file_capped(path, SIZE_CAP).map_err(|source| PostmanError::File { what: operation.clone(), source })?;
    let value = parse_capped(&bytes, &operation)?;
    let (imported, _) = super::import_value(&value)?;
    Ok(imported)
}

/// Capped JSON parse. Bare YAML text is detected and reported with the v3 instructions
/// instead of a bare syntax error.
pub(crate) fn parse_capped(bytes: &[u8], what: &str) -> Result<Value, PostmanError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| PostmanError::InvalidJson { what: what.to_string(), source: postguy_util::Error::Encoding { what: what.to_string() } })?;
    match postguy_util::json::parse_json(bytes, &Caps { max_depth: DEPTH_CAP }) {
        Ok(value) => Ok(value),
        Err(_) if looks_like_yaml(text) => Err(PostmanError::UnsupportedFormat {
            format: "Postman v3 YAML",
            instructions: "Postguy imports Postman v2 JSON; export Collection v2.1 JSON from the Postman app and import that".into(),
        }),
        Err(source) => Err(PostmanError::InvalidJson { what: what.to_string(), source }),
    }
}

fn looks_like_yaml(text: &str) -> bool {
    let head = text.trim_start();
    if head.starts_with('{') || head.starts_with('[') || head.starts_with('"') {
        return false;
    }
    text.lines().any(|line| {
        let line = line.trim();
        line.starts_with("name:") || line.starts_with("description:") || line.starts_with("_type:")
    })
}

/// Import one Postman collection document (see [`crate::postman::import_collection`]).
pub fn import_collection(v: &Value) -> Result<(Collection, super::report::ImportReport), PostmanError> {
    let mut report = super::report::ImportReport::default();
    let collection = import_collection_inner(v, &mut report)?;
    Ok((collection, report))
}

/// Import a Postman environment or globals document.
pub fn import_environment(v: &Value) -> Result<(Environment, super::report::ImportReport), PostmanError> {
    let mut report = super::report::ImportReport::default();
    let environment = import_environment_inner(v, &mut report)?;
    Ok((environment, report))
}

pub(crate) fn import_environment_inner(v: &Value, report: &mut super::report::ImportReport) -> Result<Environment, PostmanError> {
    let Some(object) = v.as_object() else {
        return Err(PostmanError::NotPostman { what: "expected an object `{...}`; an environment needs `name` and `values[]`".into() });
    };
    if !object.contains_key("values") {
        return Err(PostmanError::NotPostman {
            what: "no `values[]` list: this is not an environment/globals document (collections import with the collection importer)".into(),
        });
    }
    match object.get("_postman_variable_scope").and_then(Value::as_str) {
        Some(scope) if scope.eq_ignore_ascii_case("globals") => report.globals += 1,
        Some(scope) if !scope.eq_ignore_ascii_case("environment") => {
            report.normal("_postman_variable_scope", format!("unknown scope `{scope}`; imported as an environment"));
            report.environments += 1;
        }
        _ => report.environments += 1,
    }
    let mut extras = Map::new();
    let name = object.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let variables = match_variable_rows(object.get("values").and_then(Value::as_array), "values", report);
    extras.extend(unknown_keys_of(object, &["name", "values", "_postman_variable_scope"]));
    Ok(Environment { id: postguy_model::Id::default(), name, color: None, variables, extras: Extras(extras) })
}

fn import_collection_inner(v: &Value, report: &mut super::report::ImportReport) -> Result<Collection, PostmanError> {
    let Some(object) = v.as_object() else {
        return Err(PostmanError::NotPostman { what: "expected an object `{...}`, got another JSON type".into() });
    };
    if object.contains_key("requests") && object.contains_key("folders") && !object.contains_key("item") {
        return Err(v1_error());
    }
    let Some(info) = object.get("info").and_then(Value::as_object) else {
        return Err(PostmanError::NotPostman {
            what: "a collection needs an `info` object (name, schema) and an `item` list; this document does not have `info`".into(),
        });
    };
    let schema = info.get("schema").and_then(Value::as_str).unwrap_or("").to_string();
    if !schema.is_empty() {
        if schema.contains("collection/v1") || schema.contains("json/v1") {
            return Err(v1_error());
        }
        if !schema.contains("collection/v2") {
            return Err(PostmanError::UnsupportedFormat {
                format: "a collection schema other than v2.0/v2.1",
                instructions: "Postguy imports Collection v2.0 and v2.1 JSON; export the collection in one of those formats".into(),
            });
        }
    }
    let Some(items) = object.get("item").and_then(Value::as_array) else {
        return Err(PostmanError::NotPostman {
            what: "no `item` list: this may be an environment/globals file; use the shape-sniffing import".into(),
        });
    };

    report.collections += 1;
    let mut extras = Map::new();
    if schema.is_empty() {
        report.normal("info.schema", "missing; assumed Collection v2.1");
    } else {
        extras.insert(reserved::SCHEMA.into(), Value::String(schema));
    }
    let id = match info.get("_postman_id") {
        Some(Value::String(text)) => {
            extras.insert(reserved::ID_PROVIDED.into(), Value::Bool(true));
            postguy_model::Id(text.clone())
        }
        Some(Value::Object(guid)) => {
            extras.insert(reserved::ID_PROVIDED.into(), Value::Bool(true));
            postguy_model::Id(guid.get("guid").and_then(Value::as_str).unwrap_or_default().to_string())
        }
        _ => {
            report.normal("info._postman_id", "missing; a fresh id was generated and the id is not re-emitted on export");
            postguy_model::Id::default()
        }
    };
    if info.get("name").and_then(Value::as_str).is_none() {
        report.normal("info.name", "missing; imported with an empty name");
    }
    let info_unknown = unknown_keys_of(info, &["name", "description", "_postman_id", "schema"]);
    if !info_unknown.is_empty() {
        extras.insert(reserved::INFO.into(), Value::Object(info_unknown));
    }
    let name = info.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let description_source = info.get("description").or_else(|| object.get("description"));
    let description_at_root = info.get("description").is_none() && object.get("description").is_some();
    let description = extract_description(description_source, &mut extras, "info.description", report);
    if description_at_root {
        extras.insert(reserved::DESCRIPTION_AT_ROOT.into(), Value::Bool(true));
    }
    match object.get("protocolProfileBehavior") {
        Some(ppb) if ppb.is_object() => {
            extras.insert(reserved::COLLECTION_PPB.into(), ppb.clone());
        }
        Some(_) => report.normal("protocolProfileBehavior", "was not an object; dropped"),
        None => {}
    }
    let items = map_items(items, report);
    let auth_model = object.get("auth").map(|auth| postman_to_auth(auth, report, "auth")).unwrap_or(AuthConfig::Inherit);
    let auth = if matches!(auth_model, AuthConfig::Inherit) { None } else { Some(auth_model) };
    let scripts = match_scripts(object.get("event").and_then(Value::as_array), "event", report);
    let variables = match_variable_rows(object.get("variable").and_then(Value::as_array), "variable", report);
    extras.extend(unknown_keys_of(object, &["info", "item", "auth", "event", "variable", "protocolProfileBehavior", "description"]));
    Ok(Collection { id, name, description, variables, auth, scripts, pinned_environment: None, favourite: false, items, extras: Extras(extras) })
}

fn v1_error() -> PostmanError {
    PostmanError::UnsupportedFormat {
        format: "Postman Collection Format v1",
        instructions: "convert the collection to Collection Format v2 first (any Postman app version imports v1 and exports v2 JSON)".into(),
    }
}

// ---------- shared row helpers ----------

/// `enabled` of a row: missing is enabled; `disabled: true` means disabled.
fn is_enabled(object: &Map<String, Value>) -> bool {
    match object.get("disabled") {
        Some(Value::Bool(disabled)) => !disabled,
        _ => object.get("enabled").and_then(Value::as_bool).unwrap_or(true),
    }
}

/// Scalar cell → text (strings verbatim, numbers/booleans as their JSON text).
fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(bool) => bool.to_string(),
        _ => String::new(),
    }
}

fn optional_scalar(object: &Map<String, Value>, key: &str) -> String {
    object.get(key).map_or_else(String::new, scalar)
}

/// `description` of a row/document: string → the model string; object (`{content, type,...}`)
/// → `content` in the model string and the OBJECT in the reserved extras slot.
fn extract_description(description: Option<&Value>, extras: &mut Map<String, Value>, path: &str, report: &mut super::report::ImportReport) -> String {
    match description {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(object)) => {
            extras.insert(reserved::DESCRIPTION.into(), Value::Object(object.clone()));
            report.normal(path, "description was an object; its `content` is stored and the object kept for the round trip");
            optional_scalar(object, "content")
        }
        Some(_) => {
            report.normal(path, "description was neither a string nor an object; empty assumed");
            String::new()
        }
        None => String::new(),
    }
}

/// The keys of `object` that are NOT one of the mapped keys (extras channel candidates).
pub(crate) fn unknown_keys_of(object: &Map<String, Value>, mapped: &[&str]) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, value) in object {
        if !mapped.contains(&key.as_str()) {
            out.insert(key.clone(), value.clone());
        }
    }
    out
}

/// Build one `KeyValue` row from a Postman row (`{key, value, disabled/enabled,
/// description, ...}`), leftovers into extras.
fn row(object: &Map<String, Value>, mapped: &[&str], path: &str, report: &mut super::report::ImportReport) -> KeyValue {
    let key = optional_scalar(object, "key");
    let value = optional_scalar(object, "value");
    let mut extras = unknown_keys_of(object, mapped);
    let description = extract_description(object.get("description"), &mut extras, &format!("{path}.description"), report);
    let content_type = object.get("contentType").and_then(Value::as_str).map(str::to_string);
    KeyValue { key, value, description, enabled: is_enabled(object), kind: KeyValueKind::Text, content_type, secret: false, extras: Extras(extras) }
}

/// Variable rows (collection/folder `variable[]`, environment `values[]`).
pub(crate) fn match_variable_rows(rows: Option<&Vec<Value>>, path: &str, report: &mut super::report::ImportReport) -> Vec<Variable> {
    let mut out = Vec::new();
    for (index, raw) in rows.unwrap_or(&Vec::new()).iter().enumerate() {
        let path = format!("{path}[{index}]");
        let Some(object) = raw.as_object() else {
            report.drop(&path, "variable row is not an object; skipped");
            continue;
        };
        let secret = object.get("type").and_then(Value::as_str).is_some_and(|t| t.eq_ignore_ascii_case("secret"));
        let mut extras = unknown_keys_of(object, &["key", "value", "type", "enabled", "disabled", "description"]);
        let description = extract_description(object.get("description"), &mut extras, &path, report);
        out.push(Variable {
            key: optional_scalar(object, "key"),
            value: optional_scalar(object, "value"),
            description,
            enabled: is_enabled(object),
            secret,
            extras: Extras(extras),
        });
    }
    out
}

/// Scripts from `event[]` (`listen`: `prerequest`/`test`; unknown values drop with a report).
pub(crate) fn match_scripts(events: Option<&Vec<Value>>, path: &str, report: &mut super::report::ImportReport) -> Scripts {
    let mut scripts = Scripts::default();
    for (index, event) in events.unwrap_or(&Vec::new()).iter().enumerate() {
        let path = format!("{path}[{index}]");
        let Some(object) = event.as_object() else {
            report.drop(&path, "event is not an object; skipped");
            continue;
        };
        let listen = object.get("listen").and_then(Value::as_str).unwrap_or("");
        if !matches!(listen, "prerequest" | "test") {
            report.drop(&path, format!("`listen: {listen:?}` is not a phase this model carries (prerequest/test); the event is not imported"));
            continue;
        }
        let script_object = object.get("script").and_then(Value::as_object);
        let mut text = String::new();
        if let Some(exec) = script_object.and_then(|s| s.get("exec")).and_then(Value::as_array) {
            for (line_index, line) in exec.iter().enumerate() {
                if line_index > 0 {
                    text.push('\n');
                }
                text.push_str(&scalar(line));
            }
        } else if script_object.is_some_and(|s| s.contains_key("exec")) {
            report.normal(&path, "script `exec` was not a list; the script text is empty");
        }
        let mut script_extras = script_object.map_or_else(Map::new, |s| unknown_keys_of(s, &["exec"]));
        // One empty string as the whole `exec` is a real (degenerate) script; the joined
        // text cannot distinguish it from "no exec", so a flag keeps the round trip exact.
        if script_object
            .and_then(|s| s.get("exec"))
            .and_then(Value::as_array)
            .is_some_and(|exec| exec.len() == 1 && exec[0].as_str().unwrap_or("x").is_empty())
        {
            script_extras.insert(reserved::EXEC_EMPTY.into(), Value::Bool(true));
        }
        let script = Script { text, extras: Extras(script_extras) };
        if listen == "prerequest" {
            scripts.prerequest = Some(script);
        } else {
            scripts.test = Some(script);
        }
    }
    scripts
}

// ---------- item tree ----------

fn map_items(items: &[Value], report: &mut super::report::ImportReport) -> Vec<Item> {
    items.iter().enumerate().filter_map(|(index, item)| map_item(index, item, report)).collect()
}

fn map_item(index: usize, item: &Value, report: &mut super::report::ImportReport) -> Option<Item> {
    let path = format!("item[{index}]");
    let Some(object) = item.as_object() else {
        report.drop(&path, "item is not an object; skipped");
        return None;
    };
    if let Some(request) = object.get("request") {
        return Some(Item::Request(Box::new(map_request_item(object, path, request, report))));
    }
    if object.get("item").is_some_and(Value::is_array) {
        report.folders += 1;
        let mut extras = Map::new();
        let auth_model = object.get("auth").map(|auth| postman_to_auth(auth, report, "folder.auth")).unwrap_or(AuthConfig::Inherit);
        let auth = if matches!(auth_model, AuthConfig::Inherit) { None } else { Some(auth_model) };
        let scripts = match_scripts(object.get("event").and_then(Value::as_array), "folder.event", report);
        let variables = match_variable_rows(object.get("variable").and_then(Value::as_array), "folder.variable", report);
        let description = extract_description(object.get("description"), &mut extras, "folder.description", report);
        extras.extend(unknown_keys_of(object, &["name", "item", "description", "event", "auth", "variable", "protocolProfileBehavior"]));
        let name = object.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let nested = map_items(object.get("item").and_then(Value::as_array).unwrap_or(&Vec::new()), report);
        return Some(Item::Folder(Box::new(Folder {
            id: postguy_model::Id::default(),
            name,
            description,
            variables,
            auth,
            scripts,
            items: nested,
            extras: Extras(extras),
        })));
    }
    report.drop(&path, "item has neither `request` nor `item[]`; nothing to map it to");
    None
}

/// The HTTP request computation shared by request items and example `originalRequest`
/// snapshots (snapshots must not bump the request counter). Returns `false` when the
/// protocol name was not `http` (the report then knows the item is best-effort).
fn map_http_request_core(http: &mut HttpRequest, request: &Value, path: &str, report: &mut super::report::ImportReport) {
    let mut extras = Map::new();
    match request.as_object() {
        None => {
            // The bare-URL-string shortcut.
            extras.insert(reserved::URL_IS_STRING.into(), Value::Bool(true));
            extras.insert(reserved::REQUEST_WAS_STRING.into(), Value::Bool(true));
            http.url = request.as_str().unwrap_or("").to_string();
            let url_text = http.url.clone();
            fill_url_views(&url_text, http);
            report.normal(path, "request was a bare URL string; imported as a GET");
        }
        Some(request_object) => {
            if let Some(Value::String(protocol)) = request_object.get("protocol")
                && !protocol.eq_ignore_ascii_case("http")
            {
                report.drop(
                    &format!("{path}.request"),
                    format!("request protocol `{protocol}`: only HTTP requests are imported so far (gRPC/WebSocket items are not imported yet)"),
                );
            }
            http.method = match_method(request_object.get("method"), &mut extras, report);
            match_url(request_object.get("url"), http, &mut extras, &format!("{path}.request.url"), report);
            match_header_rows(request_object.get("header").and_then(Value::as_array), &mut http.headers, &format!("{path}.request.header"), report);
            match_body(request_object.get("body"), &mut http.body, &mut extras, &format!("{path}.request.body"), report);
            http.auth =
                request_object.get("auth").map(|auth| postman_to_auth(auth, report, &format!("{path}.request.auth"))).unwrap_or(AuthConfig::Inherit);
            http.scripts = match_scripts(request_object.get("event").and_then(Value::as_array), &format!("{path}.request.event"), report);
            http.settings = match_settings(
                request_object.get("protocolProfileBehavior"),
                &mut extras,
                &format!("{path}.request.protocolProfileBehavior"),
                report,
            );
            if request_object.contains_key("protocolProfileBehavior") && request_object.get("protocolProfileBehavior").is_some_and(Value::is_object) {
                extras.insert(reserved::PPB_AT_REQUEST_LEVEL.into(), Value::Bool(true));
            }
            extras.extend(unknown_keys_of(
                request_object,
                &["method", "url", "header", "body", "auth", "event", "protocolProfileBehavior", "options", "description"],
            ));
            if let Some(request_description) = request_object.get("description") {
                // The request-level description joins the item description (which wins if
                // the item also carries one); kept verbatim for the round trip.
                extras.insert(reserved::REQUEST_DESCRIPTION.into(), request_description.clone());
            }
            if let Some(options) = request_object.get("options") {
                // The whole request-level options object rides verbatim; the export emits
                // it exactly (the body's own options live at body level).
                extras.insert(reserved::OPTIONS_EXTRA.into(), options.clone());
            }
        }
    }
    http.extras = Extras(extras);
}

fn map_request_item(
    object: &Map<String, Value>,
    path: String,
    request: &Value,
    report: &mut super::report::ImportReport,
) -> postguy_model::collection::Request {
    let mut http = HttpRequest::default();
    map_http_request_core(&mut http, request, &path, report);
    // Per-item `auth` (Postman items may carry it beside the request): the request-level
    // block wins, the item-level one is the fallback.
    if http.auth == AuthConfig::Inherit
        && let Some(item_auth) = object.get("auth")
    {
        http.auth = postman_to_auth(item_auth, report, &format!("{path}.auth"));
        if !matches!(http.auth, AuthConfig::Inherit) {
            // Round trip level: emitted beside the request on export, as the file had it.
            http.extras.0.insert(reserved::AUTH_AT_ITEM_LEVEL.into(), Value::Bool(true));
        }
    }
    // Per-item `protocolProfileBehavior` lives at the ITEM level in the schema; when the
    // item carries one it is authoritative over a request-level block (other writers).
    if let Some(item_ppb) = object.get("protocolProfileBehavior") {
        let mut ppb_extras = Map::new();
        let mapped = match_settings(Some(item_ppb), &mut ppb_extras, &format!("{path}.protocolProfileBehavior"), report);
        // Keep the request-level unknown slot if it exists, else put the item one under it.
        let item_unknown = ppb_extras.get(reserved::ITEM_PPB);
        if let Some(Value::Object(item_unknown_map)) = item_unknown {
            if let Some(Value::Object(existing)) = http.extras.0.get_mut(reserved::ITEM_PPB) {
                let mut merged = existing.clone();
                for (key, value) in item_unknown_map {
                    merged.insert(key.clone(), value.clone());
                }
                http.extras.0.insert(reserved::ITEM_PPB.into(), Value::Object(merged));
            } else {
                http.extras.0.insert(reserved::ITEM_PPB.into(), Value::Object(item_unknown_map.clone()));
            }
        }
        // Item-level known keys override the request-level settings.
        http.settings = mapped;
        http.extras.0.shift_remove(reserved::PPB_AT_REQUEST_LEVEL);
    }
    let mut item_extras =
        unknown_keys_of(object, &["name", "request", "response", "description", "event", "auth", "variable", "protocolProfileBehavior"]);
    item_extras.extend(std::mem::take(&mut http.extras.0));
    let request_description = item_extras.shift_remove(reserved::REQUEST_DESCRIPTION);
    let description_at_request_level = !object.contains_key("description");
    let description = if object.contains_key("description") {
        extract_description(object.get("description"), &mut item_extras, &format!("{path}.description"), report)
    } else {
        // The request-level description (string or object form) is the fallback.
        match request_description {
            Some(Value::String(text)) => text,
            Some(Value::Object(object_value)) => {
                item_extras.insert(reserved::DESCRIPTION.into(), Value::Object(object_value.clone()));
                optional_scalar(&object_value, "content")
            }
            Some(_) => {
                report.normal(&format!("{path}.request.description"), "description was neither a string nor an object; empty assumed");
                String::new()
            }
            None => String::new(),
        }
    };
    report.requests += 1;
    if description_at_request_level && !description.is_empty() {
        item_extras.insert(reserved::REQUEST_DESCRIPTION_LEVEL.into(), Value::Bool(true));
    }
    postguy_model::collection::Request {
        id: postguy_model::Id::default(),
        name: object.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
        description,
        kind: RequestKind::Http(http),
        examples: map_examples(object.get("response"), &path, report),
        extras: Extras(item_extras),
    }
}

fn match_method(method: Option<&Value>, extras: &mut Map<String, Value>, report: &mut super::report::ImportReport) -> postguy_model::HttpMethod {
    let Some(text) = method.and_then(Value::as_str) else {
        if method.is_some() {
            report.normal("request.method", "method was not a string; GET assumed");
        }
        return postguy_model::HttpMethod::Get;
    };
    match text.to_ascii_uppercase().as_str() {
        "GET" => postguy_model::HttpMethod::Get,
        "POST" => postguy_model::HttpMethod::Post,
        "PUT" => postguy_model::HttpMethod::Put,
        "PATCH" => postguy_model::HttpMethod::Patch,
        "DELETE" => postguy_model::HttpMethod::Delete,
        "HEAD" => postguy_model::HttpMethod::Head,
        "OPTIONS" => postguy_model::HttpMethod::Options,
        _ => {
            extras.insert(reserved::METHOD.into(), Value::String(text.to_string()));
            report.normal("request.method", format!("method `{text}` is not in the v1 verb set; stored verbatim"));
            postguy_model::HttpMethod::Get
        }
    }
}

/// URL: string → verbatim string; object → `raw` string + query/variable rows + extras for
/// unknown object keys ([`reserved::URL_EXTRA`]).
fn match_url(url: Option<&Value>, http: &mut HttpRequest, extras: &mut Map<String, Value>, path: &str, report: &mut super::report::ImportReport) {
    let Some(url) = url else {
        extras.insert(reserved::URL_ABSENT.into(), Value::Bool(true));
        return;
    };
    match url {
        Value::String(text) => {
            extras.insert(reserved::URL_IS_STRING.into(), Value::Bool(true));
            http.url = text.clone();
            fill_url_views(text, http);
        }
        Value::Object(object) => {
            match object.get("raw").and_then(Value::as_str) {
                Some(text) => {
                    http.url = text.to_string();
                    // The two-way views follow the ROW lists when the object carries them;
                    // otherwise they derive from the string text.
                    if object.contains_key("query") {
                        match_kv_rows(object.get("query").and_then(Value::as_array), &mut http.params, &format!("{path}.query"), report);
                    } else {
                        fill_url_views(text, http);
                    }
                }
                None => {
                    let assembled = assemble_raw(object);
                    report.normal(path, "url object had no `raw`; assembled from protocol/host/path parts");
                    http.url = assembled.clone();
                    fill_url_views(&assembled, http);
                    match_kv_rows(object.get("query").and_then(Value::as_array), &mut http.params, &format!("{path}.query"), report);
                }
            }
            match_url_variable_rows(object.get("variable").and_then(Value::as_array), &mut http.path_vars, &format!("{path}.variable"), report);
            let extra = unknown_keys_of(object, &["raw", "protocol", "host", "port", "path", "query", "variable", "hash"]);
            // Fidelity of SHAPE: which of the known keys the file explicitly carried.
            let present: std::vec::Vec<&str> = ["raw", "protocol", "host", "port", "path", "query", "variable", "hash"]
                .iter()
                .copied()
                .filter(|key| object.contains_key(*key))
                .collect();
            extras.insert(reserved::URL_KEYS.into(), Value::String(present.join(",")));
            if !extra.is_empty() {
                extras.insert(reserved::URL_EXTRA.into(), Value::Object(extra));
            }
        }
        _ => {
            report.drop(path, "url was neither a string nor an object; no URL imported");
            extras.insert(reserved::URL_ABSENT.into(), Value::Bool(true));
        }
    }
}

/// Fill the params/path-vars two-way views from a URL string (postguy-urlkit parses).
fn fill_url_views(text: &str, http: &mut HttpRequest) {
    let url = postguy_urlkit::Url::parse(text);
    if let Some(query) = &url.query {
        for param in postguy_urlkit::params_of_query(&query.0) {
            http.params.push(KeyValue { key: param.key.clone(), value: param.value.clone(), ..KeyValue::default() });
        }
    }
    for name in url.path_vars() {
        http.path_vars.push(KeyValue { key: name, value: String::new(), ..KeyValue::default() });
    }
}

/// Assemble a URL text from the object's parts when `raw` is missing (best effort).
fn assemble_raw(object: &Map<String, Value>) -> String {
    let protocol = object.get("protocol").and_then(Value::as_str).unwrap_or("");
    let host = object.get("host").map_or_else(String::new, segments);
    let port = object.get("port").and_then(Value::as_str).unwrap_or("");
    let path = object.get("path").map_or_else(String::new, segments);
    let hash = object.get("hash").and_then(Value::as_str).unwrap_or("");
    let mut out = String::new();
    if !protocol.is_empty() {
        out.push_str(protocol.trim_end_matches("://"));
        out.push_str("://");
    }
    out.push_str(&host);
    if !port.is_empty() {
        out.push(':');
        out.push_str(port);
    }
    if !path.is_empty() {
        if !path.starts_with('/') {
            out.push('/');
        }
        out.push_str(&path);
    }
    if let Some(Value::Array(rows)) = object.get("query") {
        let mut first = true;
        for row_item in rows {
            let key = row_item.get("key").map_or_else(String::new, scalar);
            let value = row_item.get("value").map_or_else(String::new, scalar);
            if !first {
                out.push('&');
            }
            first = false;
            out.push_str(&key);
            out.push('=');
            out.push_str(&value);
        }
    }
    if !hash.is_empty() {
        out.push('#');
        out.push_str(hash);
    }
    out
}

/// Host/path `["api", "example", "com"]` or `"api.example.com"` → "api.example.com".
fn segments(value: &Value) -> String {
    match value {
        Value::Array(parts) => parts.iter().map(scalar).collect::<Vec<String>>().join("."),
        Value::String(text) => text.clone(),
        _ => String::new(),
    }
}

fn match_kv_rows(rows: Option<&Vec<Value>>, into: &mut Vec<KeyValue>, path: &str, report: &mut super::report::ImportReport) {
    let Some(rows) = rows else { return };
    for (index, raw) in rows.iter().enumerate() {
        let Some(object) = raw.as_object() else {
            report.drop(&format!("{path}[{index}]"), "row is not an object; skipped");
            continue;
        };
        into.push(row(object, &["key", "value", "disabled", "description"], &format!("{path}[{index}]"), report));
    }
}

fn match_url_variable_rows(rows: Option<&Vec<Value>>, into: &mut Vec<KeyValue>, path: &str, report: &mut super::report::ImportReport) {
    let Some(rows) = rows else { return };
    for (index, raw) in rows.iter().enumerate() {
        let Some(object) = raw.as_object() else {
            report.drop(&format!("{path}[{index}]"), "url variable row is not an object; skipped");
            continue;
        };
        let mut kv = row(object, &["key", "id", "value", "disabled", "description"], &format!("{path}[{index}]"), report);
        if kv.key.is_empty() {
            kv.key = optional_scalar(object, "id");
        }
        into.push(kv);
    }
}

fn match_header_rows(rows: Option<&Vec<Value>>, into: &mut Vec<KeyValue>, path: &str, report: &mut super::report::ImportReport) {
    let Some(rows) = rows else { return };
    for (index, raw) in rows.iter().enumerate() {
        let Some(object) = raw.as_object() else {
            report.drop(&format!("{path}[{index}]"), "header row is not an object; skipped");
            continue;
        };
        // `type` rides the extras channel so a literal type round trips.
        into.push(row(object, &["key", "value", "disabled", "description"], &format!("{path}[{index}]"), report));
    }
}

/// The language from a body `options` object; unknown spellings stay verbatim through the
/// reserved options slot and the model stores Text.
fn raw_language_of(options: &Value, path: &str, report: &mut super::report::ImportReport) -> RawLanguage {
    let language = options.get("raw").and_then(Value::as_object).and_then(|raw| raw.get("language")).and_then(Value::as_str);
    match language {
        Some("json") => RawLanguage::Json,
        Some("javascript") => RawLanguage::JavaScript,
        Some("html") => RawLanguage::Html,
        Some("xml") => RawLanguage::Xml,
        Some("text") | None => RawLanguage::Text,
        Some(other) => {
            report.normal(
                &format!("{path}.options.raw.language"),
                format!("language `{other}` is not in the dropdown set; kept verbatim, text assumed"),
            );
            RawLanguage::Text
        }
    }
}

// ---------- bodies ----------

fn match_body(body: Option<&Value>, into: &mut Body, extras: &mut Map<String, Value>, path: &str, report: &mut super::report::ImportReport) {
    let Some(object) = body.and_then(Value::as_object) else {
        if body.is_some() {
            report.drop(path, "body was not an object; no body imported");
        }
        return;
    };
    let mode = object.get("mode").and_then(Value::as_str).unwrap_or("");
    match mode {
        "raw" => {
            let text = optional_scalar(object, "raw");
            let mut leftovers = unknown_keys_of(object, &["mode", "raw", "options"]);
            // The language lives in `options.raw.language`; the ORIGINAL options object is
            // kept verbatim so unknown languages and unknown options keys round trip, and
            // the export prefers it over the regenerated form.
            let language = match object.get("options") {
                Some(options @ Value::Object(_)) => {
                    leftovers.insert(reserved::BODY_OPTIONS.into(), options.clone());
                    raw_language_of(options, path, report)
                }
                _ => RawLanguage::Text,
            };
            *into = Body::Raw { language, text, extras: Extras(leftovers) };
        }
        "urlencoded" => {
            let mut rows = Vec::new();
            match_kv_rows(object.get("urlencoded").and_then(Value::as_array), &mut rows, &format!("{path}.urlencoded"), report);
            // The `UrlEncoded` variant has no extras slot of its own: unknown keys of the
            // body object (e.g. its `options` sub-block) ride the reserved request-level
            // slot and are re-spread into the body object on export.
            match_text_mode_leftovers(object, extras, &["mode", "urlencoded", "options"], report);
            *into = Body::UrlEncoded(rows);
        }
        "formdata" => {
            let mut rows = Vec::new();
            match_formdata_rows(object.get("formdata").and_then(Value::as_array), &mut rows, &format!("{path}.formdata"), report);
            match_text_mode_leftovers(object, extras, &["mode", "formdata", "options"], report);
            *into = Body::FormData(rows);
        }
        "file" => match_file_body(object, into, path, report),
        "graphql" => match_graphql_body(object, into, path, report),
        "none" => report.normal(path, "body mode `none`; imported as no body"),
        other => report.drop(path, format!("body `{other}` is not one of raw/urlencoded/formdata/file/graphql; no body imported")),
    }
}

/// Unknown keys of a urlencoded/formdata body object → the request-level reserved slot the
/// exporter re-spreads into the body object.
fn match_text_mode_leftovers(
    object: &Map<String, Value>,
    extras: &mut Map<String, Value>,
    mapped: &[&str],
    report: &mut super::report::ImportReport,
) {
    let leftovers = unknown_keys_of(object, mapped);
    if leftovers.is_empty() {
        return;
    }
    report.normal("request.body", "unknown body keys kept in the reserved extras slot and re-emitted on export");
    match extras.get_mut(reserved::BODY_EXTRA) {
        Some(Value::Object(slot)) => slot.extend(leftovers),
        _ => {
            extras.insert(reserved::BODY_EXTRA.into(), Value::Object(leftovers));
        }
    }
}

fn match_file_body(object: &Map<String, Value>, into: &mut Body, path: &str, report: &mut super::report::ImportReport) {
    let file = object.get("file").and_then(Value::as_object);
    let src = file.and_then(|file| file.get("src")).map_or_else(String::new, |src| match src {
        Value::String(text) => text.clone(),
        // v2.1 allows a one-element list.
        Value::Array(parts) => parts.first().map_or_else(String::new, scalar),
        _ => String::new(),
    });
    if src.is_empty() {
        report.normal(path, "file body had no `src`; imported with an empty path");
    }
    let leftovers = file.map_or_else(Map::new, |file| unknown_keys_of(file, &["src"]));
    *into = Body::Binary { path: src, extras: Extras(leftovers) };
}

fn match_graphql_body(object: &Map<String, Value>, into: &mut Body, path: &str, report: &mut super::report::ImportReport) {
    let graphql = object.get("graphql").and_then(Value::as_object);
    let (mut query, mut variables) = (String::new(), String::new());
    let mut operation_name = None;
    let mut leftovers = unknown_keys_of(object, &["mode", "graphql", "options"]);
    if let Some(graphql) = graphql {
        query = optional_scalar(graphql, "query");
        match graphql.get("variables") {
            Some(Value::String(text)) => variables = text.clone(),
            Some(Value::Object(variables_object)) => {
                variables = serde_json::to_string(&Value::Object(variables_object.clone())).unwrap_or_default();
                leftovers.insert(reserved::VARIABLES_WAS_OBJECT.into(), Value::Bool(true));
            }
            Some(_) => report.normal(&format!("{path}.variables"), "GraphQL variables were neither a string nor an object; empty assumed"),
            None => {}
        }
        if let Some(name) = graphql.get("operationName").and_then(Value::as_str) {
            operation_name = Some(name.to_string());
        }
        leftovers.extend(unknown_keys_of(graphql, &["query", "variables", "operationName"]));
    }
    *into = Body::GraphQL { query, operation_name, variables, extras: Extras(leftovers) };
}

fn match_formdata_rows(rows: Option<&Vec<Value>>, into: &mut Vec<KeyValue>, path: &str, report: &mut super::report::ImportReport) {
    let Some(rows) = rows else { return };
    let known: &'static [&'static str] = &["key", "value", "src", "type", "disabled", "description", "contentType"];
    for (index, raw) in rows.iter().enumerate() {
        let Some(object) = raw.as_object() else {
            report.drop(&format!("{path}[{index}]"), "formdata row is not an object; skipped");
            continue;
        };
        let mut kv = row(object, known, &format!("{path}[{index}]"), report);
        let file_row = object.get("type").and_then(Value::as_str).is_some_and(|kind| kind.eq_ignore_ascii_case("file"))
            || (object.contains_key("src") && !object.contains_key("value"));
        if file_row {
            kv.kind = KeyValueKind::File;
            kv.value = object.get("src").map_or_else(String::new, |src| match src {
                Value::String(text) => text.clone(),
                Value::Array(parts) => parts.iter().map(scalar).collect::<Vec<String>>().join(";"),
                _ => String::new(),
            });
        } else {
            kv.kind = KeyValueKind::Text;
        }
        into.push(kv);
    }
}

// ---------- per-item settings (protocolProfileBehavior) ----------

/// Known `protocolProfileBehavior` keys → `RequestSettings`; unknown keys go to the
/// reserved extras slot. `sslNoVerify` is an alias that only applies without `strictSSL`.
pub(crate) fn match_settings(
    ppb: Option<&Value>,
    extras: &mut Map<String, Value>,
    path: &str,
    report: &mut super::report::ImportReport,
) -> RequestSettings {
    let Some(ppb) = ppb.and_then(Value::as_object) else {
        if ppb.is_some() {
            report.normal(path, "protocolProfileBehavior was not an object; defaults kept");
        }
        return RequestSettings::default();
    };
    let mut settings = RequestSettings::default();
    let mut unknown = Map::new();
    for (key, value) in ppb {
        match key.as_str() {
            "followRedirects" => {
                if let Some(bool) = value.as_bool() {
                    settings.follow_redirects = bool;
                } else {
                    unknown.insert(key.clone(), value.clone());
                    report.normal(path, "`followRedirects` was not a boolean (Postman also ships \"-1\"); kept as-is");
                }
            }
            "followOriginalHttpMethod" => {
                if let Some(bool) = value.as_bool() {
                    settings.follow_original_method = bool;
                } else {
                    unknown.insert(key.clone(), value.clone());
                    report.normal(path, "`followOriginalHttpMethod` was not a boolean; kept as-is");
                }
            }
            "disableUrlEncoding" => {
                if let Some(bool) = value.as_bool() {
                    settings.encode_url_automatically = !bool;
                } else {
                    unknown.insert(key.clone(), value.clone());
                }
            }
            "maxRedirects" => {
                if let Some(number) = value.as_u64() {
                    settings.max_redirects = u32::try_from(number).unwrap_or(u32::MAX);
                } else {
                    unknown.insert(key.clone(), value.clone());
                    report.normal(path, format!("`maxRedirects` value `{value}` is not a number; kept as-is"));
                }
            }
            "strictSSL" => {
                if let Some(bool) = value.as_bool() {
                    settings.verify_ssl = bool;
                } else {
                    unknown.insert(key.clone(), value.clone());
                }
            }
            "sslNoVerify" => {
                if !ppb.contains_key("strictSSL") {
                    settings.verify_ssl = !value.as_bool().unwrap_or(false);
                } else {
                    unknown.insert(key.clone(), value.clone());
                }
            }
            "timeout" => {
                if let Some(number) = value.as_u64() {
                    settings.timeout_ms = number;
                } else {
                    unknown.insert(key.clone(), value.clone());
                    report.normal(path, format!("`timeout` value `{value}` is not a number; kept as-is"));
                }
            }
            "httpVersion" => {
                if let Some(version) = value.as_str() {
                    match version {
                        "HTTP1" => settings.http_version = HttpVersion::Http1,
                        "HTTP2" => settings.http_version = HttpVersion::Http2,
                        "HTTP10" | "HTTP11" | "auto" => {}
                        _ => {
                            unknown.insert(key.clone(), value.clone());
                        }
                    }
                } else {
                    unknown.insert(key.clone(), value.clone());
                }
            }
            _ => {
                unknown.insert(key.clone(), value.clone());
            }
        }
    }
    if !unknown.is_empty() {
        extras.insert(reserved::ITEM_PPB.into(), Value::Object(unknown));
    }
    settings
}

// ---------- examples (response[]) ----------

fn map_examples(responses: Option<&Value>, path: &str, report: &mut super::report::ImportReport) -> Vec<Example> {
    let mut out = Vec::new();
    let Some(Value::Array(rows)) = responses else { return out };
    for (index, raw) in rows.iter().enumerate() {
        let path = format!("{path}.response[{index}]");
        let Some(object) = raw.as_object() else {
            report.drop(&path, "response example is not an object; skipped");
            continue;
        };
        let mapped: &[&str] = &["name", "originalRequest", "code", "status", "header", "cookie", "body", "_postman_previewlanguage"];
        let mut extras = unknown_keys_of(object, mapped);
        let original_request = object.get("originalRequest").map(|request| {
            let request_path = format!("{path}.originalRequest");
            let mut http = HttpRequest::default();
            map_http_request_core(&mut http, request, &request_path, report);
            RequestKind::Http(http)
        });
        let status = match object.get("code") {
            Some(Value::Number(number)) => number.as_u64().and_then(|code| u16::try_from(code).ok()),
            Some(Value::String(text)) => text.trim().parse::<u16>().ok(),
            Some(_) => {
                report.normal(&path, "`code` was neither a number nor a number-string; no status imported");
                None
            }
            None => None,
        };
        let status_text = object.get("status").and_then(Value::as_str).map(str::to_string);
        let mut headers = Vec::new();
        match_header_rows(object.get("header").and_then(Value::as_array), &mut headers, &format!("{path}.header"), report);
        let mut cookies = Vec::new();
        match_header_rows(object.get("cookie").and_then(Value::as_array), &mut cookies, &format!("{path}.cookie"), report);
        let body = object.get("body").and_then(Value::as_str).map(|text| ExampleBody {
            encoding: postguy_model::ContentEncoding::Plain,
            bytes: text.as_bytes().to_vec(),
            extras: Extras::default(),
        });
        let language = object.get("_postman_previewlanguage").and_then(Value::as_str).and_then(|language| match mapped_language_for(language) {
            Some(mapped) => Some(mapped),
            None => {
                extras.insert("_postman_previewlanguage".into(), Value::String(language.to_string()));
                report.normal(&path, format!("`_postman_previewlanguage: {language}` is not in the viewer set; kept as-is"));
                None
            }
        });
        report.examples += 1;
        out.push(Example {
            id: postguy_model::Id::default(),
            name: object.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            original_request,
            status,
            status_text,
            headers,
            cookies,
            body,
            language,
            time_ms: None,
            extras: Extras(extras),
        });
    }
    out
}

/// A mapped viewer language, `None` for anything else (kept in extras instead).
pub(crate) fn mapped_language_for(language: &str) -> Option<RawLanguage> {
    match language {
        "json" => Some(RawLanguage::Json),
        "javascript" => Some(RawLanguage::JavaScript),
        "html" => Some(RawLanguage::Html),
        "xml" => Some(RawLanguage::Xml),
        "text" => Some(RawLanguage::Text),
        _ => None,
    }
}
