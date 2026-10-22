//! Export: `postguy-model` → Postman Collection v2.1 / environment / globals JSON.
//!
//! Shapes are REGENERATED from the model (the export reflects what a user edits, not a
//! snapshot replay); the reserved extras names (`postman*`, see [`import::reserved`]) fence
//! in exactly the shape information the model cannot hold, and unknown keys re-enter the
//! document verbatim. Non-default field values only produce Postman keys, so defaults add no
//! noise; enum spellings return in the canonical Postman spelling (documented in `auth`).

use serde_json::{Map, Value};

use postguy_model::{
    AuthConfig, Body, Collection, Environment, Example, Folder, HttpRequest, Item, KeyValue, KeyValueKind, RawLanguage, RequestKind, Script, Scripts,
    Variable,
};

use super::auth::auth_to_postman;
use super::import::reserved;

/// Export a collection to a Postman Collection v2.1 JSON document.
#[must_use]
pub fn export_collection(model: &Collection) -> Value {
    let mut info = Map::new();
    info.insert("name".into(), Value::String(model.name.clone()));
    if model.extras.get(reserved::ID_PROVIDED).and_then(Value::as_bool).unwrap_or(false) {
        info.insert("_postman_id".into(), Value::String(model.id.0.clone()));
    }
    match model.extras.get(reserved::SCHEMA) {
        Some(schema @ Value::String(_)) => info.insert("schema".into(), schema.clone()),
        _ => info.insert("schema".into(), Value::String("https://schema.getpostman.com/json/collection/v2.1.0/collection.json".to_string())),
    };
    let description_at_root = model.extras.get(reserved::DESCRIPTION_AT_ROOT).and_then(Value::as_bool).unwrap_or(false);
    if description_at_root {
        // Recorded for the collection's own out-map, not info.
    } else {
        export_description(&model.description, &model.extras, &mut info);
    }
    if let Some(Value::Object(unknown)) = model.extras.get(reserved::INFO) {
        for (key, value) in unknown.clone() {
            info.insert(key, value);
        }
    }
    let mut out = Map::new();
    if description_at_root && let Some(object_form) = model.extras.get(reserved::DESCRIPTION) {
        out.insert("description".into(), object_form.clone());
    }
    out.insert("info".into(), Value::Object(info));
    if let Some(auth) = export_auth_option(model.auth.as_ref()) {
        out.insert("auth".into(), auth);
    }
    if let Some(event) = export_events(&model.scripts) {
        out.insert("event".into(), event);
    }
    if !model.variables.is_empty() {
        out.insert("variable".into(), export_variables(&model.variables));
    }
    if let Some(ppb @ Value::Object(_)) = model.extras.get(reserved::COLLECTION_PPB).cloned() {
        out.insert("protocolProfileBehavior".into(), ppb);
    }
    out.insert("item".into(), Value::Array(model.items.iter().map(export_item).collect()));
    for (key, value) in passthrough(&model.extras) {
        out.insert(key, value);
    }
    Value::Object(out)
}

/// Export an environment or globals document.
#[must_use]
pub fn export_environment(model: &Environment, scope: &str) -> Value {
    let mut out = Map::new();
    out.insert("_postman_variable_scope".into(), Value::String(if scope == "globals" { "globals" } else { "environment" }.to_string()));
    out.insert("name".into(), Value::String(model.name.clone()));
    out.insert("values".into(), export_variables(&model.variables));
    for (key, value) in passthrough(&model.extras) {
        out.insert(key, value);
    }
    Value::Object(out)
}

// ---------- items ----------

fn export_item(item: &Item) -> Value {
    match item {
        Item::Folder(folder) => export_folder(folder),
        Item::Request(request) => {
            let mut out = Map::new();
            out.insert("name".into(), Value::String(request.name.clone()));
            let request_string_url = match &request.kind {
                RequestKind::Http(http) => http.url.clone(),
                _ => String::new(),
            };
            out.insert(
                "request".into(),
                if request.extras.get(reserved::REQUEST_WAS_STRING).and_then(Value::as_bool).unwrap_or(false) {
                    Value::String(request_string_url)
                } else {
                    export_request(&request.kind, &request.extras)
                },
            );
            let auth_at_item_level = request.extras.get(reserved::AUTH_AT_ITEM_LEVEL).and_then(Value::as_bool).unwrap_or(false);
            if let RequestKind::Http(http) = &request.kind {
                if auth_at_item_level {
                    if let Some(auth) = export_auth_option(Some(&http.auth)) {
                        out.insert("auth".into(), auth);
                    }
                } else if let Some(auth) = export_auth_option(Some(&http.auth))
                    && let Some(Value::Object(request_object)) = out.get_mut("request")
                {
                    request_object.insert("auth".into(), auth);
                }
            }
            if let Some(examples) = export_examples(&request.examples) {
                out.insert("response".into(), examples);
            }
            let at_request_level = request.extras.get(reserved::REQUEST_DESCRIPTION_LEVEL).and_then(Value::as_bool).unwrap_or(false);
            if at_request_level {
                // The description belongs INSIDE the request object (as the file had it).
                let description = if let Some(object_form) = request.extras.get(reserved::DESCRIPTION) {
                    Some(object_form.clone())
                } else if !request.description.is_empty() {
                    Some(Value::String(request.description.clone()))
                } else {
                    None
                };
                if let (Some(description), Some(Value::Object(request_object))) = (description, out.get_mut("request")) {
                    request_object.insert("description".into(), description);
                }
            } else {
                export_description(&request.description, &request.extras, &mut out);
            }
            if let Some(Value::Object(unknown)) = request.extras.get(reserved::OPTIONS_EXTRA)
                && !unknown.is_empty()
            {
                out.insert("options".into(), Value::Object(unknown.clone()));
            }
            // Per-item protocolProfileBehavior: known settings regenerated (non-default
            // only), unknown keys from the import parked verbatim and spread in.
            let mut ppb = Map::new();
            if let RequestKind::Http(http) = &request.kind {
                ppb = known_ppb_from_settings(&http.settings);
            }
            if let Some(Value::Object(unknown)) = request.extras.get(reserved::ITEM_PPB) {
                for (key, value) in unknown.clone() {
                    ppb.insert(key, value);
                }
            }
            let ppb_at_request_level = request.extras.get(reserved::PPB_AT_REQUEST_LEVEL).and_then(Value::as_bool).unwrap_or(false);
            if !ppb.is_empty() {
                if ppb_at_request_level {
                    if let Some(Value::Object(request_object)) = out.get_mut("request") {
                        request_object.insert("protocolProfileBehavior".into(), Value::Object(ppb));
                    }
                } else {
                    out.insert("protocolProfileBehavior".into(), Value::Object(ppb));
                }
            }
            for (key, value) in passthrough(&request.extras) {
                out.insert(key, value);
            }
            Value::Object(out)
        }
    }
}

fn export_folder(folder: &Folder) -> Value {
    let mut out = Map::new();
    out.insert("name".into(), Value::String(folder.name.clone()));
    out.insert("item".into(), Value::Array(folder.items.iter().map(export_item).collect()));
    export_description(&folder.description, &folder.extras, &mut out);
    if let Some(auth) = export_auth_option(folder.auth.as_ref()) {
        out.insert("auth".into(), auth);
    }
    if let Some(event) = export_events(&folder.scripts) {
        out.insert("event".into(), event);
    }
    if !folder.variables.is_empty() {
        out.insert("variable".into(), export_variables(&folder.variables));
    }
    for (key, value) in passthrough(&folder.extras) {
        out.insert(key, value);
    }
    Value::Object(out)
}

fn export_request(kind: &RequestKind, shared: &postguy_model::Extras) -> Value {
    match kind {
        RequestKind::Http(http) => export_http_request(http, shared),
        RequestKind::Grpc(grpc) => {
            // Best-effort gRPC shape (the v2.1 format speaks `request.protocol`); fine
            // fidelity for these kinds is a documented gap.
            let mut out = Map::new();
            out.insert("method".into(), Value::String("POST".into()));
            out.insert("protocol".into(), Value::String("grpc".into()));
            out.insert("url".into(), Value::String(grpc.url.clone()));
            if !grpc.metadata.is_empty() {
                out.insert("header".into(), export_rows(&grpc.metadata));
            }
            if !grpc.message.is_empty() {
                let mut body = Map::new();
                body.insert("mode".into(), Value::String("raw".into()));
                body.insert("raw".into(), Value::String(grpc.message.clone()));
                out.insert("body".into(), Value::Object(body));
            }
            Value::Object(out)
        }
        RequestKind::WebSocket(ws) => {
            let mut out = Map::new();
            out.insert("method".into(), Value::String("GET".into()));
            out.insert("protocol".into(), Value::String("ws".into()));
            out.insert("url".into(), Value::String(ws.url.clone()));
            if !ws.headers.is_empty() {
                out.insert("header".into(), export_rows(&ws.headers));
            }
            Value::Object(out)
        }
        // Future kinds (the model enum is non_exhaustive): nothing to spell in Postman.
        _ => Value::Object(Map::new()),
    }
}

/// The full Postman verb spelling for an `HttpMethod`.
fn http_method_name(method: postguy_model::HttpMethod) -> &'static str {
    use postguy_model::HttpMethod::{Delete, Get, Head, Options, Patch, Post, Put};
    match method {
        Get => "GET",
        Post => "POST",
        Put => "PUT",
        Patch => "PATCH",
        Delete => "DELETE",
        Head => "HEAD",
        Options => "OPTIONS",
    }
}

/// Reserved-key lookup: the HTTP value's own extras first (example snapshots keep them
/// there), then the threaded request extras (request items drained them upward).
fn slot<'a>(model: &'a HttpRequest, shared: &'a postguy_model::Extras, key: &'static str) -> Option<&'a Value> {
    let own = model.extras.get(key);
    own.or_else(|| shared.get(key))
}

pub(crate) fn export_http_request(model: &HttpRequest, shared: &postguy_model::Extras) -> Value {
    let mut out = Map::new();
    let method =
        slot(model, shared, reserved::METHOD).and_then(Value::as_str).map_or_else(|| http_method_name(model.method).to_string(), str::to_string);
    out.insert("method".into(), Value::String(method));
    export_url(model, shared, &mut out);
    if !model.headers.is_empty() {
        out.insert("header".into(), export_rows(&model.headers));
    }
    if let Some(body) = export_body(model, shared) {
        out.insert("body".into(), body);
    }
    // NOTE: `auth` is emitted by the CALLER (the item) — Postman items place it beside the
    // request or inside it, whichever the import saw.
    if let Some(event) = export_events(&model.scripts) {
        out.insert("event".into(), event);
    }
    for (key, value) in passthrough(&model.extras) {
        out.insert(key, value);
    }
    Value::Object(out)
}

/// `url`: string form (when it arrived as a string), else the regenerated object form.
fn export_url(model: &HttpRequest, shared: &postguy_model::Extras, out: &mut Map<String, Value>) {
    if slot(model, shared, reserved::URL_ABSENT).and_then(Value::as_bool).unwrap_or(false) {
        return;
    }
    if slot(model, shared, reserved::URL_IS_STRING).and_then(Value::as_bool).unwrap_or(false) {
        out.insert("url".into(), Value::String(model.url.clone()));
        return;
    }
    let mut url_object = Map::new();
    url_object.insert("raw".into(), Value::String(model.url.clone()));
    let carried: std::vec::Vec<String> = slot(model, shared, reserved::URL_KEYS)
        .and_then(Value::as_str)
        .map_or_else(|| "raw,protocol,host,path,query,variable".to_string(), str::to_string)
        .split(',')
        .filter(|key| !key.is_empty())
        .map(str::to_string)
        .collect();
    let carries = |key: &str| carried.iter().any(|k| k == key);
    let parsed = postguy_urlkit::Url::parse(&model.url);
    if carries("protocol")
        && let Some(scheme) = &parsed.scheme
    {
        url_object.insert("protocol".into(), Value::String(scheme.clone()));
    }
    let authority = parsed.authority.split('@').next_back().unwrap_or_default();
    let (host, port) = split_port(authority);
    if carries("host") && !host.is_empty() {
        url_object.insert("host".into(), Value::Array(parts_of(&host)));
    }
    if carries("port")
        && let Some(port) = port
    {
        url_object.insert("port".into(), Value::String(port));
    }
    if carries("path") && !parsed.path.is_empty() {
        url_object.insert("path".into(), Value::Array(parts_of(parsed.path.as_str())));
    }
    if carries("query") && !model.params.is_empty() {
        url_object.insert("query".into(), export_rows(&model.params));
    }
    if carries("variable") && !model.path_vars.is_empty() {
        url_object.insert("variable".into(), export_rows(&model.path_vars));
    }
    if carries("hash")
        && let Some(fragment) = &parsed.fragment
    {
        url_object.insert("hash".into(), Value::String(fragment.clone()));
    }
    if let Some(Value::Object(unknown)) = slot(model, shared, reserved::URL_EXTRA) {
        for (key, value) in unknown.clone() {
            url_object.insert(key, value);
        }
    }
    out.insert("url".into(), Value::Object(url_object));
}

/// `host` = "example", `port` = "8080" when the authority ends in `:digits`.
fn split_port(host: &str) -> (String, Option<String>) {
    if let Some(position) = host.rfind(':') {
        let tail = &host[position + 1..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            return (host[..position].to_string(), Some(tail.to_string()));
        }
    }
    (host.to_string(), None)
}

/// Split on host/path separators; empty segments collapse.
fn parts_of(value: &str) -> Vec<Value> {
    value.split('/').filter(|part| !part.is_empty()).map(|part| Value::String(part.to_string())).collect()
}

// ---------- rows, variables, descriptions ----------

/// Postman kv rows: `key`, `value` (or `src` for file rows, `type` for formdata), plus
/// non-default bits and untouched extras.
fn export_rows(rows: &[KeyValue]) -> Value {
    let mut out = Vec::new();
    for row in rows {
        let mut object = Map::new();
        object.insert("key".into(), Value::String(row.key.clone()));
        match row.kind {
            KeyValueKind::File => {
                object.insert("src".into(), Value::String(row.value.clone()));
                object.insert("type".into(), Value::String("file".into()));
            }
            KeyValueKind::Text => {
                object.insert("value".into(), Value::String(row.value.clone()));
            }
        }
        if let Some(descr) = row.extras.get(reserved::DESCRIPTION) {
            object.insert("description".into(), descr.clone());
        } else if !row.description.is_empty() {
            object.insert("description".into(), Value::String(row.description.clone()));
        }
        if !row.enabled {
            object.insert("disabled".into(), Value::Bool(true));
        }
        if let Some(content_type) = &row.content_type {
            object.insert("contentType".into(), Value::String(content_type.clone()));
        }
        for (key, value) in passthrough(&row.extras) {
            object.insert(key, value);
        }
        out.push(Value::Object(object));
    }
    Value::Array(out)
}

/// Formdata rows spell `type` on every row (Postman's shape for the mode).
fn export_formdata_rows(rows: &[KeyValue]) -> Value {
    let mut out = Vec::new();
    for row in rows {
        let mut object = Map::new();
        object.insert("key".into(), Value::String(row.key.clone()));
        match row.kind {
            KeyValueKind::File => {
                object.insert("src".into(), Value::String(row.value.clone()));
                object.insert("type".into(), Value::String("file".into()));
            }
            KeyValueKind::Text => {
                object.insert("value".into(), Value::String(row.value.clone()));
                object.insert("type".into(), Value::String("text".into()));
            }
        }
        if let Some(descr) = row.extras.get(reserved::DESCRIPTION) {
            object.insert("description".into(), descr.clone());
        } else if !row.description.is_empty() {
            object.insert("description".into(), Value::String(row.description.clone()));
        }
        if !row.enabled {
            object.insert("disabled".into(), Value::Bool(true));
        }
        if let Some(content_type) = &row.content_type {
            object.insert("contentType".into(), Value::String(content_type.clone()));
        }
        for (key, value) in passthrough(&row.extras) {
            object.insert(key, value);
        }
        out.push(Value::Object(object));
    }
    Value::Array(out)
}

/// The Postman `variable[]`/`values[]` row shape.
fn export_variables(variables: &[Variable]) -> Value {
    let mut out = Vec::new();
    for variable in variables {
        let mut object = Map::new();
        object.insert("key".into(), Value::String(variable.key.clone()));
        object.insert("value".into(), Value::String(variable.value.clone()));
        if !variable.description.is_empty() {
            object.insert("description".into(), Value::String(variable.description.clone()));
        }
        if variable.secret {
            object.insert("type".into(), Value::String("secret".into()));
        }
        if !variable.enabled {
            object.insert("disabled".into(), Value::Bool(true));
        }
        for (key, value) in passthrough(&variable.extras) {
            object.insert(key, value);
        }
        out.push(Value::Object(object));
    }
    Value::Array(out)
}

/// `description`: the object form when the import parked one, else the string (if present).
fn export_description(description: &str, extras: &postguy_model::Extras, out: &mut Map<String, Value>) {
    if let Some(object) = extras.get(reserved::DESCRIPTION) {
        out.insert("description".into(), object.clone());
        return;
    }
    if !description.is_empty() {
        out.insert("description".into(), Value::String(description.to_string()));
    }
}

fn export_auth_option(auth: Option<&AuthConfig>) -> Option<Value> {
    let auth = auth?;
    match auth {
        AuthConfig::Inherit => None,
        other => auth_to_postman(other),
    }
}

// ---------- events ----------

/// The `event[]` list for a Scripts value; `None` when no phase is present.
fn export_events(scripts: &Scripts) -> Option<Value> {
    let mut out = Vec::new();
    for (listen, script) in [("prerequest", scripts.prerequest.as_ref()), ("test", scripts.test.as_ref())] {
        if let Some(script) = script {
            out.push(export_event(listen, script));
        }
    }
    if out.is_empty() { None } else { Some(Value::Array(out)) }
}

fn export_event(listen: &str, script: &Script) -> Value {
    let mut script_object = Map::new();
    script_object.insert("type".into(), Value::String("text/javascript".into()));
    script_object.insert("exec".into(), export_exec(script));
    for (key, value) in passthrough(&script.extras) {
        script_object.insert(key, value);
    }
    let mut event = Map::new();
    event.insert("listen".into(), Value::String(listen.to_string()));
    event.insert("script".into(), Value::Object(script_object));
    Value::Object(event)
}

/// The `exec` list: joined lines split back; the degenerate `[""]` through the flag.
fn export_exec(script: &Script) -> Value {
    if script.text.is_empty() {
        if script.extras.get(reserved::EXEC_EMPTY).and_then(Value::as_bool).unwrap_or(false) {
            return Value::Array(vec![Value::String(String::new())]);
        }
        return Value::Array(Vec::new());
    }
    Value::Array(script.text.split('\n').map(|line| Value::String(line.to_string())).collect())
}

// ---------- bodies ----------

/// The Postman body object; `None` when there is no body to write.
fn export_body(model: &HttpRequest, shared: &postguy_model::Extras) -> Option<Value> {
    let mut out = Map::new();
    match &model.body {
        Body::None => return None,
        Body::FormData(rows) => {
            out.insert("mode".into(), Value::String("formdata".into()));
            out.insert("formdata".into(), export_formdata_rows(rows));
        }
        Body::UrlEncoded(rows) => {
            out.insert("mode".into(), Value::String("urlencoded".into()));
            out.insert("urlencoded".into(), export_rows(rows));
        }
        Body::Binary { path, extras } => {
            out.insert("mode".into(), Value::String("file".into()));
            let mut file = Map::new();
            if !path.is_empty() {
                file.insert("src".into(), Value::String(path.clone()));
            }
            for (key, value) in passthrough(extras) {
                file.insert(key, value);
            }
            out.insert("file".into(), Value::Object(file));
        }
        Body::GraphQL { query, operation_name, variables, extras } => {
            out.insert("mode".into(), Value::String("graphql".into()));
            let mut graphql = Map::new();
            graphql.insert("query".into(), Value::String(query.clone()));
            if extras.get(reserved::VARIABLES_WAS_OBJECT).and_then(Value::as_bool).unwrap_or(false) {
                let re_parsed: Option<Value> = serde_json::from_str(variables).ok();
                match re_parsed {
                    Some(Value::Object(object)) => graphql.insert("variables".into(), Value::Object(object)),
                    _ => graphql.insert("variables".into(), Value::String(variables.clone())),
                };
            } else {
                graphql.insert("variables".into(), Value::String(variables.clone()));
            }
            if let Some(operation_name) = operation_name {
                graphql.insert("operationName".into(), Value::String(operation_name.clone()));
            }
            for (key, value) in passthrough(extras) {
                graphql.insert(key, value);
            }
            out.insert("graphql".into(), Value::Object(graphql.clone()));
        }
        Body::Raw { language, text, extras } => {
            out.insert("mode".into(), Value::String("raw".into()));
            out.insert("raw".into(), Value::String(text.clone()));
            // The ORIGINAL options object rides verbatim (also for unknown languages).
            if let Some(options) = extras.get(reserved::BODY_OPTIONS).cloned() {
                out.insert("options".into(), options);
            } else if *language != RawLanguage::Text {
                let mut raw_options = Map::new();
                raw_options.insert("language".into(), Value::String(export_raw_language(*language)));
                let mut options = Map::new();
                options.insert("raw".into(), Value::Object(raw_options));
                out.insert("options".into(), Value::Object(options));
            }
        }
        // Future Body variants (the enum is non_exhaustive): no Postman equivalent yet.
        _ => return None,
    }
    // The urlencoded/formdata branch has no extras slot of its own; its unknown body keys
    // were parked in the request-level reserved slot and spread here.
    if let Some(Value::Object(leftovers)) = slot(model, shared, reserved::BODY_EXTRA) {
        for (key, value) in leftovers.clone() {
            out.insert(key, value.clone());
        }
    }
    Some(Value::Object(out))
}

pub(crate) fn export_raw_language(language: RawLanguage) -> String {
    match language {
        RawLanguage::Text => "text".into(),
        RawLanguage::Json => "json".into(),
        RawLanguage::Xml => "xml".into(),
        RawLanguage::Html => "html".into(),
        RawLanguage::JavaScript => "javascript".into(),
    }
}

// ---------- settings (protocolProfileBehavior) ----------

/// The KNOWN part of the per-item `protocolProfileBehavior` from `RequestSettings`: only
/// non-default fields emit (the import made the inverse assumption explicit).
pub(crate) fn known_ppb_from_settings(settings: &postguy_model::RequestSettings) -> Map<String, Value> {
    let defaults = postguy_model::RequestSettings::default();
    let mut ppb = Map::new();
    if settings.follow_redirects != defaults.follow_redirects {
        ppb.insert("followRedirects".into(), Value::Bool(settings.follow_redirects));
    }
    if settings.follow_original_method != defaults.follow_original_method {
        ppb.insert("followOriginalHttpMethod".into(), Value::Bool(settings.follow_original_method));
    }
    if settings.encode_url_automatically != defaults.encode_url_automatically {
        ppb.insert("disableUrlEncoding".into(), Value::Bool(!settings.encode_url_automatically));
    }
    if settings.max_redirects != defaults.max_redirects
        && let Ok(number) = serde_json::to_value(settings.max_redirects)
    {
        ppb.insert("maxRedirects".into(), number);
    }
    if settings.verify_ssl != defaults.verify_ssl {
        ppb.insert("strictSSL".into(), Value::Bool(settings.verify_ssl));
    }
    if settings.timeout_ms != defaults.timeout_ms
        && let Ok(number) = serde_json::to_value(settings.timeout_ms)
    {
        ppb.insert("timeout".into(), number);
    }
    ppb
}

// ---------- examples ----------

fn export_examples(examples: &[Example]) -> Option<Value> {
    if examples.is_empty() { None } else { Some(Value::Array(examples.iter().map(export_example).collect())) }
}

fn export_example(example: &Example) -> Value {
    let mut out = Map::new();
    out.insert("name".into(), Value::String(example.name.clone()));
    if let Some(RequestKind::Http(http)) = &example.original_request {
        out.insert("originalRequest".into(), export_http_request(http, &example.extras));
    }
    if let Some(status) = example.status
        && let Ok(number) = serde_json::to_value(status)
    {
        out.insert("code".into(), number);
    }
    if let Some(status_text) = &example.status_text {
        out.insert("status".into(), Value::String(status_text.clone()));
    }
    if !example.headers.is_empty() {
        out.insert("header".into(), export_rows(&example.headers));
    }
    if !example.cookies.is_empty() {
        out.insert("cookie".into(), export_rows(&example.cookies));
    }
    if let Some(body) = &example.body
        && let Ok(text) = std::str::from_utf8(&body.bytes)
    {
        out.insert("body".into(), Value::String(text.to_string()));
    }
    let preview = example.language.map(|language| Value::String(export_raw_language(language)));
    if let Some(Value::String(_)) = example.extras.get("_postman_previewlanguage") {
        if let Some(literal) = example.extras.get("_postman_previewlanguage") {
            out.insert("_postman_previewlanguage".into(), literal.clone());
        }
    } else if let Some(preview) = preview {
        out.insert("_postman_previewlanguage".into(), preview);
    }
    for (key, value) in passthrough(&example.extras) {
        out.insert(key, value);
    }
    Value::Object(out)
}

/// The passthrough extras: everything except the reserved `postman*` names.
fn passthrough(extras: &postguy_model::Extras) -> Vec<(String, Value)> {
    extras.0.iter().filter(|(key, _)| !key.starts_with("postman")).map(|(key, value)| (key.clone(), value.clone())).collect()
}
