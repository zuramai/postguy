//! OpenAPI 3.x and Swagger 2.0 import (JSON or YAML) into a Postguy collection.
//!
//! The document is a hostile input: size, nesting (YAML budget), `$ref` depth and cycles,
//! operation count and generated example size are all capped, nothing is fetched (external
//! `$ref`s are reported, never followed) and no path panics. Behaviour comes from the public
//! OpenAPI 3.0/3.1 and Swagger 2.0 specifications.

use std::collections::{BTreeMap, BTreeSet};

use postguy_model::auth::{ApiKeyAuth, BasicAuth, BearerAuth, OAuth2Auth, OAuth2Grant};
use postguy_model::{
    AuthConfig, Body, Collection, Example, ExampleBody, Folder, HttpMethod, HttpRequest, Id, Item, KeyValue, KeyValueKind, RawLanguage, Request,
    RequestKind, Variable,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::SpecError;
use crate::postman::ImportReport;

/// Largest document read (JSON or YAML).
pub const SIZE_CAP: usize = 20 << 20;
/// Most operations imported from one document.
pub const MAX_OPERATIONS: usize = 10_000;
/// Deepest `$ref` chain followed.
pub const MAX_REF_DEPTH: usize = 32;
/// Largest generated or copied example body.
pub const EXAMPLE_CAP: usize = 64 << 10;
/// Most nodes one generated example may hold (an object of objects must not explode).
const EXAMPLE_NODES: usize = 5_000;
/// Most path segments that become nested folders.
const MAX_FOLDER_DEPTH: usize = 32;
/// Most example responses imported per operation.
const MAX_EXAMPLES_PER_OPERATION: usize = 20;

const METHODS: &[(&str, HttpMethod)] = &[
    ("get", HttpMethod::Get),
    ("put", HttpMethod::Put),
    ("post", HttpMethod::Post),
    ("delete", HttpMethod::Delete),
    ("options", HttpMethod::Options),
    ("head", HttpMethod::Head),
    ("patch", HttpMethod::Patch),
];

/// How operations are grouped into folders.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FolderStrategy {
    /// One folder per first tag (untagged operations stay at the root).
    #[default]
    Tags,
    /// Nested folders per literal path segment.
    Paths,
}

/// Parse YAML into a JSON value with a budget against alias bombs and deep nesting.
pub fn parse_yaml(text: &str) -> Result<Value, SpecError> {
    if text.len() > SIZE_CAP {
        return Err(SpecError::TooLarge { what: "the YAML document", size: text.len(), cap: SIZE_CAP });
    }
    let options = serde_saphyr::options! {
        budget: serde_saphyr::budget! {
            max_depth: 100,
            max_nodes: 1_000_000,
            max_events: 4_000_000,
            max_aliases: 10_000,
            max_anchors: 10_000,
        },
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::LastWins,
    };
    serde_saphyr::from_str_with_options::<Value>(text, options).map_err(|err| {
        let mut message = err.to_string();
        if message.len() > 500 {
            let mut cut = 500;
            while !message.is_char_boundary(cut) {
                cut -= 1;
            }
            message.truncate(cut);
        }
        SpecError::Yaml { message }
    })
}

/// Whether `v` is an OpenAPI 3.x or Swagger 2.0 document.
pub fn is_openapi(v: &Value) -> bool {
    v.get("openapi").and_then(Value::as_str).is_some_and(|version| version.starts_with("3."))
        || v.get("swagger").and_then(Value::as_str) == Some("2.0")
}

/// Import an OpenAPI/Swagger document.
pub fn import_openapi(v: &Value, folders: FolderStrategy) -> Result<(Collection, ImportReport), SpecError> {
    if !is_openapi(v) {
        return Err(SpecError::Invalid { what: "the document", detail: "it has no `openapi: 3.x` or `swagger: \"2.0\"` key".to_string() });
    }
    let mut importer = Importer::new(v);
    let collection = importer.run(folders);
    Ok((collection, importer.report))
}

struct Importer<'a> {
    doc: &'a Value,
    oas3: bool,
    report: ImportReport,
    placeholders: BTreeSet<&'static str>,
    collection_scheme: Option<String>,
    cut_refs: BTreeSet<String>,
    extensions: usize,
    nodes: usize,
}

/// A placeholder variable the auth mapping refers to, with where it is used.
const AUTH_VARIABLES: &[&str] = &["username", "password", "bearerToken", "apiKey", "clientId", "clientSecret"];

impl<'a> Importer<'a> {
    fn new(doc: &'a Value) -> Importer<'a> {
        let oas3 = doc.get("openapi").is_some();
        Importer {
            doc,
            oas3,
            report: ImportReport::default(),
            placeholders: BTreeSet::new(),
            collection_scheme: None,
            cut_refs: BTreeSet::new(),
            extensions: 0,
            nodes: 0,
        }
    }

    // ---- $ref ------------------------------------------------------------------------

    /// The value a local `$ref` points at; `None` (and a report entry) for external,
    /// missing or too-deep references.
    fn lookup(&mut self, reference: &str, path: &str) -> Option<&'a Value> {
        let Some(pointer) = reference.strip_prefix('#') else {
            self.report.drop(path, format!("external $ref {reference} is not followed"));
            return None;
        };
        let decoded = postguy_urlkit::decode_percent(pointer);
        let target = self.doc.pointer(&decoded);
        if target.is_none() {
            self.report.normal(path, format!("$ref {reference} points at nothing"));
        }
        target
    }

    /// Follow a `$ref` chain to a concrete value. Cycles and over-deep chains are cut.
    fn deref(&mut self, value: &'a Value, path: &str) -> Option<&'a Value> {
        let mut current = value;
        let mut seen: Vec<&str> = Vec::new();
        while let Some(reference) = current.get("$ref").and_then(Value::as_str) {
            if seen.contains(&reference) || seen.len() >= MAX_REF_DEPTH {
                self.cut(reference, path);
                return None;
            }
            seen.push(reference);
            current = self.lookup(reference, path)?;
        }
        Some(current)
    }

    fn cut(&mut self, reference: &str, path: &str) {
        if self.cut_refs.insert(reference.to_string()) {
            self.report.normal(path, format!("recursive $ref {reference} cut"));
        }
    }

    // ---- examples --------------------------------------------------------------------

    /// A sample value for a schema, bounded in depth and node count.
    fn sample(&mut self, schema: &'a Value, path: &str) -> Value {
        self.nodes = 0;
        let mut stack = Vec::new();
        self.sample_at(schema, 0, &mut stack, path)
    }

    fn sample_at(&mut self, schema: &'a Value, depth: usize, stack: &mut Vec<String>, path: &str) -> Value {
        self.nodes = self.nodes.saturating_add(1);
        if depth > 16 || self.nodes > EXAMPLE_NODES {
            return Value::Null;
        }
        let mut current = schema;
        let mut pushed = 0usize;
        while let Some(reference) = current.get("$ref").and_then(Value::as_str) {
            if stack.iter().any(|seen| seen == reference) || pushed >= MAX_REF_DEPTH {
                self.cut(reference, path);
                stack.truncate(stack.len().saturating_sub(pushed));
                return json!({});
            }
            let Some(target) = self.lookup(reference, path) else {
                stack.truncate(stack.len().saturating_sub(pushed));
                return Value::Null;
            };
            stack.push(reference.to_string());
            pushed += 1;
            current = target;
        }
        let out = self.sample_schema(current, depth, stack, path);
        stack.truncate(stack.len().saturating_sub(pushed));
        out
    }

    fn sample_schema(&mut self, schema: &'a Value, depth: usize, stack: &mut Vec<String>, path: &str) -> Value {
        for key in ["example", "default"] {
            if let Some(value) = schema.get(key) {
                return value.clone();
            }
        }
        if let Some(first) = schema.get("enum").and_then(Value::as_array).and_then(|items| items.first()) {
            return first.clone();
        }
        if let Some(parts) = schema.get("allOf").and_then(Value::as_array) {
            let mut merged = serde_json::Map::new();
            for part in parts {
                if let Value::Object(map) = self.sample_at(part, depth + 1, stack, path) {
                    merged.extend(map);
                }
            }
            if let Some(Value::Object(own)) = schema.get("properties").map(|_| self.sample_properties(schema, depth, stack, path)) {
                merged.extend(own);
            }
            return Value::Object(merged);
        }
        for key in ["oneOf", "anyOf"] {
            if let Some(first) = schema.get(key).and_then(Value::as_array).and_then(|items| items.first()) {
                return self.sample_at(first, depth + 1, stack, path);
            }
        }
        let kind = match schema.get("type") {
            Some(Value::String(kind)) => Some(kind.as_str()),
            Some(Value::Array(kinds)) => kinds.iter().filter_map(Value::as_str).find(|kind| *kind != "null"),
            _ => None,
        };
        match kind {
            Some("object") => self.sample_properties(schema, depth, stack, path),
            None if schema.get("properties").is_some() => self.sample_properties(schema, depth, stack, path),
            Some("array") => match schema.get("items") {
                Some(items) => Value::Array(vec![self.sample_at(items, depth + 1, stack, path)]),
                None => Value::Array(Vec::new()),
            },
            Some("string") => Value::String(
                match schema.get("format").and_then(Value::as_str) {
                    Some("date-time") => "1970-01-01T00:00:00Z",
                    Some("date") => "1970-01-01",
                    Some("uuid") => "00000000-0000-0000-0000-000000000000",
                    Some("email") => "user@example.com",
                    Some("uri" | "url") => "https://example.com",
                    _ => "string",
                }
                .to_string(),
            ),
            Some("integer" | "number") => json!(0),
            Some("boolean") => Value::Bool(true),
            _ => Value::Null,
        }
    }

    fn sample_properties(&mut self, schema: &'a Value, depth: usize, stack: &mut Vec<String>, path: &str) -> Value {
        let mut map = serde_json::Map::new();
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (name, property) in properties.iter().take(100) {
                map.insert(name.clone(), self.sample_at(property, depth + 1, stack, path));
            }
        }
        Value::Object(map)
    }

    /// Pretty text of a sample, or `{}` with a report entry when it is too large.
    fn pretty(&mut self, value: &Value, path: &str) -> String {
        let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string());
        if text.len() > EXAMPLE_CAP {
            self.report.normal(path, format!("the example is over {EXAMPLE_CAP} bytes and was replaced by {{}}"));
            return "{}".to_string();
        }
        text
    }

    /// The example value of a media type object (`example`, the first `examples.*.value`, or a
    /// schema sample).
    fn media_sample(&mut self, media: &'a Value, path: &str) -> Option<Value> {
        if let Some(example) = media.get("example") {
            return Some(example.clone());
        }
        if let Some(first) = media.get("examples").and_then(Value::as_object).and_then(|map| map.values().next())
            && let Some(resolved) = self.deref(first, path)
            && let Some(value) = resolved.get("value")
        {
            return Some(value.clone());
        }
        media.get("schema").map(|schema| self.sample(schema, path))
    }

    // ---- parameters -------------------------------------------------------------------

    fn param_value(&mut self, param: &'a Value, path: &str) -> String {
        let schema = param.get("schema").unwrap_or(param);
        let resolved = self.deref(schema, path).unwrap_or(schema);
        let candidate = param
            .get("example")
            .or_else(|| resolved.get("example"))
            .or_else(|| resolved.get("default"))
            .or_else(|| resolved.get("enum").and_then(Value::as_array).and_then(|items| items.first()));
        match candidate {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Null) | None => {
                let kind = resolved.get("type").and_then(Value::as_str).unwrap_or("string");
                format!("<{kind}>")
            }
            Some(other) => other.to_string(),
        }
    }

    /// Path-level parameters under operation-level ones (same name and location: operation wins).
    fn parameters(&mut self, path_item: &'a Value, op: &'a Value, path: &str) -> Vec<&'a Value> {
        let mut out: Vec<&'a Value> = Vec::new();
        for list in [path_item.get("parameters"), op.get("parameters")] {
            let Some(items) = list.and_then(Value::as_array) else { continue };
            for item in items.iter().take(500) {
                let Some(param) = self.deref(item, path) else { continue };
                let key = (param.get("name").and_then(Value::as_str), param.get("in").and_then(Value::as_str));
                out.retain(|existing| (existing.get("name").and_then(Value::as_str), existing.get("in").and_then(Value::as_str)) != key);
                out.push(param);
            }
        }
        out
    }

    // ---- auth -------------------------------------------------------------------------

    fn scheme(&mut self, name: &str, path: &str) -> AuthConfig {
        let schemes = if self.oas3 { self.doc.pointer("/components/securitySchemes") } else { self.doc.get("securityDefinitions") };
        let Some(raw) = schemes.and_then(|map| map.get(name)) else {
            self.report.normal(path, format!("security scheme `{name}` is not defined"));
            return AuthConfig::Inherit;
        };
        let Some(scheme) = self.deref(raw, path) else { return AuthConfig::Inherit };
        let kind = scheme.get("type").and_then(Value::as_str).unwrap_or_default();
        let http = scheme.get("scheme").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase();
        match (kind, http.as_str()) {
            ("basic", _) | ("http", "basic") => {
                self.placeholders.extend(["username", "password"]);
                AuthConfig::Basic(Box::new(BasicAuth { username: "{{username}}".into(), password: "{{password}}".into(), ..BasicAuth::default() }))
            }
            ("http", "bearer") => {
                self.placeholders.insert("bearerToken");
                AuthConfig::Bearer(Box::new(BearerAuth { token: "{{bearerToken}}".into(), ..BearerAuth::default() }))
            }
            ("apiKey", _) => {
                let location = scheme.get("in").and_then(Value::as_str).unwrap_or("header");
                if location == "cookie" {
                    self.report.drop(path, "cookie API keys are not supported");
                    return AuthConfig::NoAuth;
                }
                self.placeholders.insert("apiKey");
                let location = if location == "query" { postguy_model::auth::AuthLocation::Query } else { postguy_model::auth::AuthLocation::Header };
                AuthConfig::ApiKey(Box::new(ApiKeyAuth {
                    key: scheme.get("name").and_then(Value::as_str).unwrap_or("api_key").to_string(),
                    value: "{{apiKey}}".into(),
                    location,
                    ..ApiKeyAuth::default()
                }))
            }
            ("oauth2", _) => self.oauth2(scheme, path),
            ("openIdConnect", _) => {
                self.placeholders.extend(["clientId", "clientSecret"]);
                let url = scheme.get("openIdConnectUrl").and_then(Value::as_str).unwrap_or_default();
                self.report.normal(path, format!("openIdConnect: set the auth and token URLs from {url}"));
                AuthConfig::OAuth2(Box::new(OAuth2Auth {
                    grant_type: OAuth2Grant::AuthorizationCode,
                    client_id: "{{clientId}}".into(),
                    client_secret: "{{clientSecret}}".into(),
                    ..OAuth2Auth::default()
                }))
            }
            _ => {
                self.report.drop(path, format!("security scheme `{name}` of type `{kind}` is not supported"));
                AuthConfig::Inherit
            }
        }
    }

    fn oauth2(&mut self, scheme: &'a Value, _path: &str) -> AuthConfig {
        self.placeholders.extend(["clientId", "clientSecret"]);
        let mut auth = OAuth2Auth { client_id: "{{clientId}}".into(), client_secret: "{{clientSecret}}".into(), ..OAuth2Auth::default() };
        let scopes_of = |flow: &Value| {
            flow.get("scopes").and_then(Value::as_object).map(|scopes| scopes.keys().cloned().collect::<Vec<_>>().join(" ")).unwrap_or_default()
        };
        if self.oas3 {
            let flows = scheme.get("flows");
            let pick = |name: &str| flows.and_then(|flows| flows.get(name));
            if let Some(flow) = pick("clientCredentials") {
                auth.grant_type = OAuth2Grant::ClientCredentials;
                auth.token_url = flow.get("tokenUrl").and_then(Value::as_str).unwrap_or_default().to_string();
                auth.scope = scopes_of(flow);
            } else if let Some(flow) = pick("authorizationCode") {
                auth.grant_type = OAuth2Grant::AuthorizationCode;
                auth.auth_url = flow.get("authorizationUrl").and_then(Value::as_str).unwrap_or_default().to_string();
                auth.token_url = flow.get("tokenUrl").and_then(Value::as_str).unwrap_or_default().to_string();
                auth.scope = scopes_of(flow);
            } else if let Some(flow) = pick("password") {
                auth.grant_type = OAuth2Grant::PasswordCredentials;
                auth.token_url = flow.get("tokenUrl").and_then(Value::as_str).unwrap_or_default().to_string();
                auth.scope = scopes_of(flow);
                self.placeholders.extend(["username", "password"]);
                auth.username = "{{username}}".into();
                auth.password = "{{password}}".into();
            } else if let Some(flow) = pick("implicit") {
                auth.grant_type = OAuth2Grant::Implicit;
                auth.auth_url = flow.get("authorizationUrl").and_then(Value::as_str).unwrap_or_default().to_string();
                auth.scope = scopes_of(flow);
            }
        } else {
            auth.auth_url = scheme.get("authorizationUrl").and_then(Value::as_str).unwrap_or_default().to_string();
            auth.token_url = scheme.get("tokenUrl").and_then(Value::as_str).unwrap_or_default().to_string();
            auth.scope = scopes_of(scheme);
            auth.grant_type = match scheme.get("flow").and_then(Value::as_str) {
                Some("application") => OAuth2Grant::ClientCredentials,
                Some("password") => {
                    self.placeholders.extend(["username", "password"]);
                    auth.username = "{{username}}".into();
                    auth.password = "{{password}}".into();
                    OAuth2Grant::PasswordCredentials
                }
                Some("implicit") => OAuth2Grant::Implicit,
                _ => OAuth2Grant::AuthorizationCode,
            };
        }
        AuthConfig::OAuth2(Box::new(auth))
    }

    /// The first scheme name of a `security` array's first requirement.
    fn first_scheme(security: &Value) -> Option<String> {
        security.as_array()?.first()?.as_object()?.keys().next().cloned()
    }

    // ---- the document -----------------------------------------------------------------

    fn base_url(&mut self) -> String {
        if self.oas3 {
            let Some(server) = self.doc.pointer("/servers/0") else { return String::new() };
            let mut url = server.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
            if let Some(variables) = server.get("variables").and_then(Value::as_object) {
                for (name, variable) in variables {
                    let default = variable.get("default").and_then(Value::as_str).unwrap_or_default();
                    url = url.replace(&format!("{{{name}}}"), default);
                }
            }
            if !url.is_empty() && !url.contains("://") {
                self.report.normal("servers[0].url", "servers[0].url is relative; set baseUrl");
            }
            url.trim_end_matches('/').to_string()
        } else {
            let Some(host) = self.doc.get("host").and_then(Value::as_str) else { return String::new() };
            let scheme =
                self.doc.get("schemes").and_then(Value::as_array).and_then(|schemes| schemes.first()).and_then(Value::as_str).unwrap_or("https");
            let base = self.doc.get("basePath").and_then(Value::as_str).unwrap_or_default();
            format!("{scheme}://{host}{}", base.trim_end_matches('/'))
        }
    }

    fn run(&mut self, strategy: FolderStrategy) -> Collection {
        let doc = self.doc;
        let name = doc.pointer("/info/title").and_then(Value::as_str).filter(|title| !title.trim().is_empty()).unwrap_or("Imported API").to_string();
        let description = doc.pointer("/info/description").and_then(Value::as_str).unwrap_or_default().to_string();
        let base_url = self.base_url();
        if base_url.is_empty() {
            self.report.normal("servers", "no server URL; set the baseUrl collection variable");
        }
        self.collection_scheme = doc.get("security").and_then(Self::first_scheme);
        let collection_auth = self.collection_scheme.clone().map(|scheme| self.scheme(&scheme, "security"));

        let tags: Vec<(String, String)> = doc
            .get("tags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(|tag| {
                        Some((tag.get("name")?.as_str()?.to_string(), tag.get("description").and_then(Value::as_str).unwrap_or_default().to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let tag_order: Vec<&str> = tags.iter().map(|(name, _)| name.as_str()).collect();
        let tag_descriptions: BTreeMap<String, String> = tags.iter().cloned().collect();

        let mut items: Vec<Item> = Vec::new();
        let mut operations = 0usize;
        if let Some(paths) = doc.get("paths").and_then(Value::as_object) {
            'paths: for (path, item) in paths {
                let Some(path_item) = self.deref(item, &format!("paths.{path}")) else { continue };
                let Some(entries) = path_item.as_object() else { continue };
                self.extensions += entries.keys().filter(|key| key.starts_with("x-")).count();
                for (key, op) in entries {
                    let Some((_, method)) = METHODS.iter().find(|(name, _)| name == key) else { continue };
                    if operations >= MAX_OPERATIONS {
                        self.report.normal("paths", format!("more than {MAX_OPERATIONS} operations; the rest were not imported"));
                        break 'paths;
                    }
                    operations += 1;
                    let at = format!("paths.{path}.{key}");
                    let request = self.operation(path, *method, path_item, op, &at);
                    let folder_path: Vec<String> = match strategy {
                        FolderStrategy::Tags => op
                            .get("tags")
                            .and_then(Value::as_array)
                            .and_then(|tags| tags.first())
                            .and_then(Value::as_str)
                            .map(|tag| vec![tag.to_string()])
                            .unwrap_or_default(),
                        FolderStrategy::Paths => {
                            path.split('/').filter(|segment| !segment.is_empty()).take(MAX_FOLDER_DEPTH).map(str::to_string).collect()
                        }
                    };
                    // Paths: the leaf request sits in its path's folder (all segments), the same
                    // way Postman groups by URL.
                    insert(&mut items, &folder_path, request, &mut self.report.folders, &tag_descriptions, strategy == FolderStrategy::Tags);
                }
            }
        }
        if strategy == FolderStrategy::Tags {
            let rank = |item: &Item| match item {
                Item::Folder(folder) => tag_order.iter().position(|tag| *tag == folder.name).unwrap_or(usize::MAX - 1),
                Item::Request(_) => usize::MAX,
            };
            items.sort_by_key(rank);
        }
        if doc.get("webhooks").is_some() {
            self.report.drop("webhooks", "webhooks are not imported");
        }
        let extensions = self.extensions + count_extensions(doc);
        if extensions > 0 {
            self.report.normal("x-*", format!("{extensions} vendor extensions (x-*) were not imported"));
        }

        let mut variables = vec![Variable { key: "baseUrl".into(), value: base_url, ..Variable::default() }];
        for key in AUTH_VARIABLES {
            if self.placeholders.contains(key) {
                variables.push(Variable { key: (*key).to_string(), value: String::new(), ..Variable::default() });
            }
        }
        self.report.collections = 1;
        Collection { id: Id::generate(), name, description, variables, auth: collection_auth, items, ..Collection::default() }
    }

    fn operation(&mut self, path: &str, method: HttpMethod, path_item: &'a Value, op: &'a Value, at: &str) -> Request {
        let key = METHODS.iter().find(|(_, candidate)| *candidate == method).map(|(name, _)| name.to_ascii_uppercase()).unwrap_or_default();
        let text = |field: &str| op.get(field).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty());
        let mut name = text("summary").or_else(|| text("operationId")).map(str::to_string).unwrap_or_else(|| format!("{key} {path}"));
        if name.chars().count() > 200 {
            name = name.chars().take(200).collect();
        }
        let description = op.get("description").and_then(Value::as_str).unwrap_or_default().to_string();
        if op.get("callbacks").is_some() {
            self.report.drop(at, "callbacks are not imported");
        }
        self.extensions += op.as_object().map(|map| map.keys().filter(|key| key.starts_with("x-")).count()).unwrap_or(0);

        let mut http = HttpRequest { method, url: format!("{{{{baseUrl}}}}{}", colon_path(path)), ..HttpRequest::default() };
        let mut cookies: Vec<String> = Vec::new();
        let mut form_rows: Vec<(String, String, bool)> = Vec::new();
        let mut body_param: Option<&'a Value> = None;
        for param in self.parameters(path_item, op, at) {
            let name = param.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            if name.is_empty() {
                continue;
            }
            let description = param.get("description").and_then(Value::as_str).unwrap_or_default().to_string();
            let required = param.get("required").and_then(Value::as_bool).unwrap_or(false);
            let value = self.param_value(param, at);
            let row = |enabled: bool| KeyValue {
                key: name.clone(),
                value: value.clone(),
                description: description.clone(),
                enabled,
                ..KeyValue::default()
            };
            match param.get("in").and_then(Value::as_str).unwrap_or_default() {
                "path" => http.path_vars.push(row(true)),
                "query" => http.params.push(row(required)),
                "header" => {
                    if !matches!(name.to_ascii_lowercase().as_str(), "accept" | "content-type" | "authorization") {
                        http.headers.push(row(required));
                    }
                }
                "cookie" => cookies.push(format!("{name}={value}")),
                "body" => body_param = Some(param),
                "formData" => form_rows.push((
                    name.clone(),
                    if param.get("type").and_then(Value::as_str) == Some("file") { String::new() } else { value },
                    param.get("type").and_then(Value::as_str) == Some("file"),
                )),
                _ => {}
            }
        }
        if !cookies.is_empty() {
            self.report.normal(at, "cookie parameters became a Cookie header");
            http.headers.push(KeyValue { key: "Cookie".into(), value: cookies.join("; "), ..KeyValue::default() });
        }

        let media = self.request_body(&mut http, op, body_param, &form_rows, at);
        if let Some(media) = media
            && !http.headers.iter().any(|row| row.key.eq_ignore_ascii_case("content-type"))
        {
            http.headers.push(KeyValue { key: "Content-Type".into(), value: media, ..KeyValue::default() });
        }

        // Auth: absent security inherits; an empty array switches auth off.
        http.auth = match op.get("security") {
            None => AuthConfig::Inherit,
            Some(security) if security.as_array().is_some_and(Vec::is_empty) => AuthConfig::NoAuth,
            Some(security) => match Self::first_scheme(security) {
                Some(scheme) if Some(&scheme) == self.collection_scheme.as_ref() => AuthConfig::Inherit,
                Some(scheme) => self.scheme(&scheme, at),
                None => AuthConfig::Inherit,
            },
        };

        let examples = self.examples(op, at);
        self.report.requests += 1;
        self.report.examples += examples.len();
        Request { id: Id::generate(), name, description, kind: RequestKind::Http(http), examples, ..Request::default() }
    }

    /// Fill the body of `http`; returns the media type chosen (for Content-Type).
    fn request_body(
        &mut self,
        http: &mut HttpRequest,
        op: &'a Value,
        body_param: Option<&'a Value>,
        form_rows: &[(String, String, bool)],
        at: &str,
    ) -> Option<String> {
        if self.oas3 {
            let body = self.deref(op.get("requestBody")?, at)?;
            let content = body.get("content")?.as_object()?;
            let is_json = |media: &str| media == "application/json" || media.ends_with("+json");
            let pick = content
                .iter()
                .find(|(media, _)| is_json(media))
                .or_else(|| content.iter().find(|(media, _)| *media == "application/x-www-form-urlencoded"))
                .or_else(|| content.iter().find(|(media, _)| *media == "multipart/form-data"))
                .or_else(|| content.iter().find(|(media, _)| *media == "application/xml" || media.starts_with("text/")))
                .or_else(|| content.iter().next())?;
            let (media_type, media) = pick;
            let sample = self.media_sample(media, at);
            self.set_body(http, media_type, sample, media.get("schema"), at);
            Some(media_type.clone())
        } else if let Some(param) = body_param {
            let consumes = self.consumes(op);
            let media = consumes
                .iter()
                .find(|media| media.contains("json"))
                .or_else(|| consumes.first())
                .cloned()
                .unwrap_or_else(|| "application/json".to_string());
            let sample = param.get("schema").map(|schema| self.sample(schema, at));
            self.set_body(http, &media, sample, param.get("schema"), at);
            Some(media)
        } else if !form_rows.is_empty() {
            let consumes = self.consumes(op);
            let multipart = consumes.iter().any(|media| media == "multipart/form-data") || form_rows.iter().any(|(_, _, file)| *file);
            let rows: Vec<KeyValue> = form_rows
                .iter()
                .map(|(key, value, file)| KeyValue {
                    key: key.clone(),
                    value: value.clone(),
                    kind: if *file { KeyValueKind::File } else { KeyValueKind::Text },
                    ..KeyValue::default()
                })
                .collect();
            if multipart {
                http.body = Body::FormData(rows);
                Some("multipart/form-data".to_string())
            } else {
                http.body = Body::UrlEncoded(rows);
                Some("application/x-www-form-urlencoded".to_string())
            }
        } else {
            None
        }
    }

    fn consumes(&self, op: &Value) -> Vec<String> {
        op.get("consumes")
            .or_else(|| self.doc.get("consumes"))
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn set_body(&mut self, http: &mut HttpRequest, media_type: &str, sample: Option<Value>, schema: Option<&'a Value>, at: &str) {
        if media_type == "application/json" || media_type.ends_with("+json") {
            let text = match &sample {
                Some(value) => self.pretty(value, at),
                None => String::new(),
            };
            http.body = Body::Raw { language: RawLanguage::Json, text, extras: Default::default() };
        } else if media_type == "application/x-www-form-urlencoded" || media_type == "multipart/form-data" {
            let resolved = schema.and_then(|schema| self.deref(schema, at));
            let required: Vec<&str> = resolved
                .and_then(|schema| schema.get("required"))
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let mut rows = Vec::new();
            if let Some(properties) = resolved.and_then(|schema| schema.get("properties")).and_then(Value::as_object) {
                for (name, property) in properties.iter().take(200) {
                    let property_schema = self.deref(property, at).unwrap_or(property);
                    let file = property_schema.get("format").and_then(Value::as_str) == Some("binary")
                        || property_schema.get("type").and_then(Value::as_str) == Some("file");
                    let value = if file { String::new() } else { self.param_value(property, at) };
                    rows.push(KeyValue {
                        key: name.clone(),
                        value,
                        description: property_schema.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
                        enabled: required.contains(&name.as_str()) || required.is_empty(),
                        kind: if file { KeyValueKind::File } else { KeyValueKind::Text },
                        ..KeyValue::default()
                    });
                }
            }
            http.body = if media_type == "multipart/form-data" { Body::FormData(rows) } else { Body::UrlEncoded(rows) };
        } else {
            let text = match sample {
                Some(Value::String(text)) => text,
                Some(Value::Null) | None => String::new(),
                Some(other) => self.pretty(&other, at),
            };
            let language = if media_type.contains("xml") {
                RawLanguage::Xml
            } else if media_type.contains("html") {
                RawLanguage::Html
            } else {
                RawLanguage::Text
            };
            http.body = Body::Raw { language, text, extras: Default::default() };
        }
    }

    fn examples(&mut self, op: &'a Value, at: &str) -> Vec<Example> {
        let Some(responses) = op.get("responses").and_then(Value::as_object) else { return Vec::new() };
        let mut out = Vec::new();
        for (code, raw) in responses.iter().take(MAX_EXAMPLES_PER_OPERATION) {
            let here = format!("{at}.responses.{code}");
            let Some(response) = self.deref(raw, &here) else { continue };
            if response.get("links").is_some() {
                self.report.drop(&here, "links are not imported");
            }
            let description = response.get("description").and_then(Value::as_str).unwrap_or_default().trim().to_string();
            let (media_type, sample) = if self.oas3 {
                match response.get("content").and_then(Value::as_object) {
                    Some(content) => {
                        let pick = content.iter().find(|(media, _)| media.contains("json")).or_else(|| content.iter().next());
                        match pick {
                            Some((media, value)) => (Some(media.clone()), self.media_sample(value, &here)),
                            None => (None, None),
                        }
                    }
                    None => (None, None),
                }
            } else {
                let produces: Vec<&str> = op
                    .get("produces")
                    .or_else(|| self.doc.get("produces"))
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let media = produces
                    .iter()
                    .find(|media| media.contains("json"))
                    .or_else(|| produces.first())
                    .map(|media| (*media).to_string())
                    .unwrap_or_else(|| "application/json".to_string());
                let sample = response
                    .get("examples")
                    .and_then(Value::as_object)
                    .and_then(|examples| examples.get(&media).or_else(|| examples.values().next()))
                    .cloned()
                    .or_else(|| response.get("schema").map(|schema| self.sample(schema, &here)));
                (sample.as_ref().map(|_| media), sample)
            };
            let Some(sample) = sample else { continue };
            let json_media = media_type.as_deref().is_some_and(|media| media.contains("json"));
            let text = match sample {
                Value::String(text) if !json_media => text,
                other => self.pretty(&other, &here),
            };
            let status = code.parse::<u16>().ok();
            let name = if description.is_empty() { code.clone() } else { format!("{code} {description}") };
            let headers =
                media_type.map(|media| vec![KeyValue { key: "Content-Type".into(), value: media, ..KeyValue::default() }]).unwrap_or_default();
            out.push(Example {
                id: Id::generate(),
                name,
                status,
                headers,
                body: Some(ExampleBody { bytes: text.into_bytes(), ..ExampleBody::default() }),
                language: Some(if json_media { RawLanguage::Json } else { RawLanguage::Text }),
                ..Example::default()
            });
        }
        out
    }
}

/// `/pets/{id}` as `/pets/:id`.
fn colon_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let (head, tail) = rest.split_at(open);
        out.push_str(head);
        match tail.find('}') {
            Some(close) => {
                out.push(':');
                out.push_str(tail.get(1..close).unwrap_or_default());
                rest = tail.get(close + 1..).unwrap_or_default();
            }
            None => {
                out.push_str(tail);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Top-level vendor extensions.
fn count_extensions(doc: &Value) -> usize {
    doc.as_object().map(|map| map.keys().filter(|key| key.starts_with("x-")).count()).unwrap_or(0)
}

/// Put `request` into the folder `path`, creating folders as needed (capped depth).
fn insert(
    items: &mut Vec<Item>,
    path: &[String],
    request: Request,
    folders_made: &mut usize,
    descriptions: &BTreeMap<String, String>,
    describe: bool,
) {
    let mut current = items;
    for name in path.iter().take(MAX_FOLDER_DEPTH) {
        let existing = current.iter().position(|item| matches!(item, Item::Folder(folder) if folder.name == *name));
        let at = match existing {
            Some(at) => at,
            None => {
                let description = if describe { descriptions.get(name).cloned().unwrap_or_default() } else { String::new() };
                current.push(Item::Folder(Box::new(Folder { id: Id::generate(), name: name.clone(), description, ..Folder::default() })));
                *folders_made = folders_made.saturating_add(1);
                current.len().saturating_sub(1)
            }
        };
        let step = current;
        current = match step.get_mut(at) {
            Some(Item::Folder(folder)) => &mut folder.items,
            _ => return,
        };
    }
    current.push(Item::Request(Box::new(request)));
}
