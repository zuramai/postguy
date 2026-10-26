//! Bruno folder import: `.bru` files plus `bruno.json`, `environments/*.bru` and a `.env` file
//! become a Postguy collection, environments and an import report.
//!
//! Behaviour comes from the public Bruno documentation (<https://docs.usebruno.com>): the Bru
//! language, the tag reference and the assertions page. The function is pure: the caller walks
//! the directory (so file-system caps live with the caller) and hands over the `.bru` texts.
//!
//! Mapping notes:
//! - `{{process.env.X}}` becomes `{{X}}` everywhere; every such `X` becomes a secret variable of
//!   every environment, valued from the `.env` file when it names it.
//! - Folder and collection `headers` have no model field, so they are copied onto each descendant
//!   request that does not set the header, with a warning.
//! - Request-level `vars:pre-request` become `pg.variables.set(...)` lines at the top of the
//!   pre-request script; `vars:post-response` become `pg.setVar(...)` lines at the end of the
//!   post-response script.
//! - `@file(path)` multipart values become file rows holding `{{% file('path') %}}`, which
//!   reads the file from the collection's Assets.

pub mod lang;

use std::collections::{BTreeMap, BTreeSet};

use postguy_model::auth::{ApiKeyAuth, AuthLocation, BasicAuth, BearerAuth, DigestAuth};
use postguy_model::{
    AssertOperator, Assertion, AuthConfig, Body, Collection, Environment, Extras, Folder, HttpMethod, HttpRequest, Id, Item, KeyValue, KeyValueKind,
    RawLanguage, Request, RequestKind, RequestTests, Script, Scripts, Variable,
};

use self::lang::{BlockBody, BruDoc, DictEntry};
use crate::error::SpecError;
use crate::postman::ImportReport;

/// Deepest directory nesting the importer follows.
const MAX_DEPTH: usize = 64;

/// One file handed to the importer: a path relative to the Bruno collection folder (`/`
/// separated) and its text.
#[derive(Debug, Clone)]
pub struct BrunoFile {
    pub path: String,
    pub text: String,
}

/// What a Bruno import produced.
#[derive(Debug, Clone)]
pub struct BrunoImport {
    pub collection: Collection,
    pub environments: Vec<Environment>,
    pub report: ImportReport,
    /// Names read through `{{process.env.X}}`.
    pub process_env: BTreeSet<String>,
}

struct Ctx {
    report: ImportReport,
    process_env: BTreeSet<String>,
}

impl Ctx {
    /// Rewrite `{{process.env.X}}` to `{{X}}`, collecting `X`.
    fn re(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(open) = rest.find("{{") {
            out.push_str(rest.get(..open).unwrap_or_default());
            let after = rest.get(open + 2..).unwrap_or_default();
            if let Some(close) = after.find("}}") {
                let inner = after.get(..close).unwrap_or_default().trim();
                if let Some(name) = inner.strip_prefix("process.env.")
                    && !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    self.process_env.insert(name.to_string());
                    out.push_str("{{");
                    out.push_str(name);
                    out.push_str("}}");
                    rest = after.get(close + 2..).unwrap_or_default();
                    continue;
                }
            }
            out.push_str("{{");
            rest = after;
        }
        out.push_str(rest);
        out
    }
}

/// A request file awaiting mapping (its folder headers are known only when the tree is built).
struct RequestSource {
    path: String,
    doc: BruDoc,
    seq: Option<u64>,
    name: String,
}

#[derive(Default)]
struct Dir {
    folder: Option<(String, BruDoc)>,
    requests: Vec<RequestSource>,
    dirs: BTreeMap<String, Dir>,
}

fn seq_of(doc: &BruDoc) -> Option<u64> {
    doc.value("meta", "seq").and_then(|seq| seq.trim().parse().ok())
}

fn file_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".bru").unwrap_or(name).to_string()
}

/// Import a Bruno collection folder.
pub fn import_bruno(files: &[BrunoFile], dotenv: &BTreeMap<String, String>) -> Result<BrunoImport, SpecError> {
    let mut ctx = Ctx { report: ImportReport { collections: 1, ..ImportReport::default() }, process_env: BTreeSet::new() };
    let mut name = String::new();
    let mut root_collection: Option<(String, BruDoc)> = None;
    let mut root = Dir::default();
    let mut env_files: Vec<(String, BruDoc)> = Vec::new();

    for file in files {
        if file.path == "bruno.json" {
            match serde_json::from_str::<serde_json::Value>(&file.text) {
                Ok(value) => name = value.get("name").and_then(serde_json::Value::as_str).unwrap_or_default().trim().to_string(),
                Err(error) => ctx.report.normal("bruno.json", format!("could not be read ({error}); the collection name is a default")),
            }
            continue;
        }
        if !file.path.ends_with(".bru") {
            continue;
        }
        let parts: Vec<&str> = file.path.split('/').collect();
        if parts.len() > MAX_DEPTH {
            ctx.report.drop(&file.path, "nested too deeply");
            continue;
        }
        let doc = match lang::parse(&file.text) {
            Ok(doc) => doc,
            Err(error) => {
                ctx.report.drop(&file.path, error.to_string());
                continue;
            }
        };
        if parts.first() == Some(&"environments") && parts.len() == 2 {
            env_files.push((file_stem(&file.path), doc));
            continue;
        }
        let Some((leaf, dirs)) = parts.split_last() else { continue };
        let mut dir = &mut root;
        for part in dirs {
            dir = dir.dirs.entry((*part).to_string()).or_default();
        }
        match *leaf {
            "collection.bru" if dirs.is_empty() => root_collection = Some((file.path.clone(), doc)),
            "collection.bru" => ctx.report.drop(&file.path, "collection.bru is only read at the collection root"),
            "folder.bru" => dir.folder = Some((file.path.clone(), doc)),
            _ => {
                let request_name =
                    doc.value("meta", "name").map(str::trim).filter(|n| !n.is_empty()).map_or_else(|| file_stem(&file.path), str::to_string);
                dir.requests.push(RequestSource { path: file.path.clone(), seq: seq_of(&doc), name: request_name, doc });
            }
        }
    }

    if name.is_empty() {
        name = "Bruno collection".to_string();
    }
    let mut collection = Collection { id: Id::generate(), name, ..Collection::default() };
    let mut level = Level::default();
    if let Some((path, doc)) = &root_collection {
        level.apply(doc, path, &mut ctx);
    }
    if let Some((path, doc)) = &root.folder {
        level.apply(doc, path, &mut ctx);
    }
    let inherited = level.headers.clone();
    collection.description = level.description;
    collection.variables = level.variables;
    collection.auth = level.auth;
    collection.scripts = level.scripts;
    let top = std::mem::take(&mut root);
    collection.items = build_children(top, &inherited, &mut ctx);

    let mut environments = Vec::new();
    env_files.sort_by(|a, b| a.0.cmp(&b.0));
    for (env_name, doc) in &env_files {
        environments.push(map_environment(env_name, doc, &mut ctx));
    }
    ctx.report.environments = environments.len();
    let process_env = std::mem::take(&mut ctx.process_env);
    let mut missing_values = Vec::new();
    for key in &process_env {
        let value = dotenv.get(key);
        if value.is_none() {
            missing_values.push(key.clone());
        }
        for environment in &mut environments {
            if environment.variables.iter().any(|variable| &variable.key == key) {
                continue;
            }
            environment.variables.push(Variable { key: key.clone(), value: value.cloned().unwrap_or_default(), secret: true, ..Variable::default() });
        }
    }
    if !missing_values.is_empty() {
        ctx.report.normal(".env", format!("no value for process.env names: {}; set them as secrets in each environment", missing_values.join(", ")));
    }
    Ok(BrunoImport { collection, environments, report: ctx.report, process_env })
}

/// What a collection- or folder-level file contributes.
#[derive(Default)]
struct Level {
    description: String,
    variables: Vec<Variable>,
    auth: Option<AuthConfig>,
    scripts: Scripts,
    headers: Vec<KeyValue>,
}

const LEVEL_TAGS: &[&str] =
    &["meta", "headers", "auth", "vars:pre-request", "vars:post-response", "script:pre-request", "script:post-response", "docs"];

impl Level {
    fn apply(&mut self, doc: &BruDoc, path: &str, ctx: &mut Ctx) {
        for block in &doc.blocks {
            if !LEVEL_TAGS.contains(&block.tag.as_str()) && !block.tag.starts_with("auth:") {
                ctx.report.drop(&format!("{path}: {}", block.tag), "not supported");
            }
        }
        if let Some(docs) = doc.text("docs") {
            self.description = ctx.re(docs);
        }
        for entry in doc.dict("vars:pre-request").unwrap_or_default() {
            let value = ctx.re(&entry.value);
            self.variables.retain(|variable| variable.key != entry.key);
            self.variables.push(Variable {
                key: entry.key.clone(),
                value,
                enabled: entry.enabled,
                description: entry.description.clone().unwrap_or_default(),
                ..Variable::default()
            });
        }
        if let Some(mode) = doc.value("auth", "mode") {
            match auth_of(mode.trim(), doc, path, ctx) {
                AuthConfig::Inherit => {}
                auth => self.auth = Some(auth),
            }
        }
        if let Some(text) = doc.text("script:pre-request") {
            append_script(&mut self.scripts.prerequest, text);
        }
        if let Some(text) = doc.text("script:post-response") {
            append_script(&mut self.scripts.test, text);
        }
        let post_vars = post_response_lines(doc, ctx);
        if !post_vars.is_empty() {
            append_script(&mut self.scripts.test, &post_vars);
        }
        let mut added = false;
        for entry in doc.dict("headers").unwrap_or_default() {
            if !entry.enabled || entry.key.is_empty() {
                continue;
            }
            added = true;
            let value = ctx.re(&entry.value);
            self.headers.retain(|header| !header.key.eq_ignore_ascii_case(&entry.key));
            self.headers.push(KeyValue { key: entry.key.clone(), value, ..KeyValue::default() });
        }
        if added {
            ctx.report.normal(path, "headers have no folder-level field; they are copied onto each request below that does not set them");
        }
    }
}

fn append_script(slot: &mut Option<Script>, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    match slot {
        Some(existing) if !existing.text.trim().is_empty() => {
            existing.text.push('\n');
            existing.text.push_str(text);
        }
        _ => *slot = Some(Script { text: text.to_string(), extras: Extras::default() }),
    }
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

fn pre_request_lines(doc: &BruDoc, ctx: &mut Ctx) -> String {
    let mut out = String::new();
    for entry in doc.dict("vars:pre-request").unwrap_or_default() {
        if entry.enabled && !entry.key.is_empty() {
            let value = ctx.re(&entry.value);
            out.push_str(&format!("pg.variables.set({}, {});\n", json_string(&entry.key), json_string(&value)));
        }
    }
    out
}

fn post_response_lines(doc: &BruDoc, ctx: &mut Ctx) -> String {
    let mut out = String::new();
    for entry in doc.dict("vars:post-response").unwrap_or_default() {
        if entry.enabled && !entry.key.is_empty() {
            let expr = ctx.re(&entry.value);
            out.push_str(&format!("pg.setVar({}, ({}));\n", json_string(&entry.key), expr));
        }
    }
    out
}

fn auth_of(mode: &str, doc: &BruDoc, path: &str, ctx: &mut Ctx) -> AuthConfig {
    let mut field = |tag: &str, key: &str| -> String { ctx.re(doc.value(tag, key).unwrap_or_default()) };
    match mode {
        "" | "inherit" => AuthConfig::Inherit,
        "none" => AuthConfig::NoAuth,
        "basic" => AuthConfig::Basic(Box::new(BasicAuth {
            username: field("auth:basic", "username"),
            password: field("auth:basic", "password"),
            ..BasicAuth::default()
        })),
        "bearer" => AuthConfig::Bearer(Box::new(BearerAuth { token: field("auth:bearer", "token"), ..BearerAuth::default() })),
        "digest" => AuthConfig::Digest(Box::new(DigestAuth {
            username: field("auth:digest", "username"),
            password: field("auth:digest", "password"),
            ..DigestAuth::default()
        })),
        "apikey" => {
            let placement = field("auth:apikey", "placement");
            let location = if placement.eq_ignore_ascii_case("queryparams") { AuthLocation::Query } else { AuthLocation::Header };
            AuthConfig::ApiKey(Box::new(ApiKeyAuth {
                key: field("auth:apikey", "key"),
                value: field("auth:apikey", "value"),
                location,
                ..ApiKeyAuth::default()
            }))
        }
        other => {
            ctx.report.drop(path, format!("auth mode `{other}` is not supported"));
            AuthConfig::Unknown { kind: other.to_string(), extras: Extras::default() }
        }
    }
}

fn build_children(dir: Dir, inherited: &[KeyValue], ctx: &mut Ctx) -> Vec<Item> {
    let mut entries: Vec<(Option<u64>, String, Item)> = Vec::new();
    for source in dir.requests {
        let (seq, name) = (source.seq, source.name.clone());
        if let Some(request) = map_request(source, inherited, ctx) {
            entries.push((seq, name, Item::Request(Box::new(request))));
        }
    }
    for (dir_name, child) in dir.dirs {
        let seq = child.folder.as_ref().and_then(|(_, doc)| seq_of(doc));
        let folder = build_folder(child, &dir_name, inherited, ctx);
        entries.push((seq, folder.name.clone(), Item::Folder(Box::new(folder))));
    }
    entries.sort_by(|a, b| (a.0.unwrap_or(u64::MAX), a.1.to_lowercase(), &a.1).cmp(&(b.0.unwrap_or(u64::MAX), b.1.to_lowercase(), &b.1)));
    entries.into_iter().map(|(_, _, item)| item).collect()
}

fn build_folder(dir: Dir, dir_name: &str, inherited: &[KeyValue], ctx: &mut Ctx) -> Folder {
    ctx.report.folders += 1;
    let mut level = Level::default();
    let mut name = dir_name.to_string();
    if let Some((path, doc)) = &dir.folder {
        level.apply(doc, path, ctx);
        if let Some(meta_name) = doc.value("meta", "name").map(str::trim).filter(|n| !n.is_empty()) {
            name = meta_name.to_string();
        }
    }
    let mut merged = level.headers.clone();
    for header in inherited {
        if !merged.iter().any(|existing| existing.key.eq_ignore_ascii_case(&header.key)) {
            merged.push(header.clone());
        }
    }
    let items = build_children(dir, &merged, ctx);
    Folder {
        id: Id::generate(),
        name,
        description: level.description,
        variables: level.variables,
        auth: level.auth,
        scripts: level.scripts,
        items,
        extras: Extras::default(),
    }
}

const REQUEST_TAGS: &[&str] = &[
    "meta",
    "get",
    "post",
    "put",
    "patch",
    "delete",
    "options",
    "head",
    "params:query",
    "params:path",
    "headers",
    "body",
    "auth",
    "vars:pre-request",
    "vars:post-response",
    "script:pre-request",
    "script:post-response",
    "tests",
    "docs",
    "assert",
];

fn rows(entries: &[DictEntry], ctx: &mut Ctx) -> Vec<KeyValue> {
    entries
        .iter()
        .map(|entry| KeyValue {
            key: ctx.re(&entry.key),
            value: ctx.re(&entry.value),
            description: entry.description.clone().unwrap_or_default(),
            enabled: entry.enabled,
            ..KeyValue::default()
        })
        .collect()
}

fn asset_shortcode(path: &str) -> String {
    let escaped = path.replace('\\', "\\\\").replace('\'', "\\'");
    format!("{{{{% file('{escaped}') %}}}}")
}

/// The asset paths of an `@file(a|b)` value, as collection-relative `/` paths.
fn file_refs(value: &str) -> Option<Vec<String>> {
    let inner = value.trim().strip_prefix("@file(")?.strip_suffix(')')?;
    Some(
        inner
            .split('|')
            .map(|part| part.trim().replace('\\', "/"))
            .map(|part| part.strip_prefix("./").map_or(part.clone(), str::to_string))
            .filter(|part| !part.is_empty())
            .collect(),
    )
}

fn map_body(mode: &str, doc: &BruDoc, path: &str, ctx: &mut Ctx) -> Body {
    match mode {
        "" | "none" => Body::None,
        "json" | "text" | "xml" | "sparql" => {
            let tag = format!("body:{mode}");
            let text = doc.text(&tag).or_else(|| if mode == "json" { doc.text("body") } else { None }).unwrap_or_default();
            let language = match mode {
                "json" => RawLanguage::Json,
                "xml" => RawLanguage::Xml,
                _ => RawLanguage::Text,
            };
            Body::Raw { language, text: ctx.re(text), extras: Extras::default() }
        }
        "form-urlencoded" | "formUrlEncoded" => Body::UrlEncoded(rows(doc.dict("body:form-urlencoded").unwrap_or_default(), ctx)),
        "multipart-form" | "multipartForm" => {
            let mut out = Vec::new();
            for entry in doc.dict("body:multipart-form").unwrap_or_default() {
                if let Some(paths) = file_refs(&entry.value) {
                    for asset in paths {
                        ctx.report.normal(path, format!("import `{asset}` into Assets"));
                        out.push(KeyValue {
                            key: ctx.re(&entry.key),
                            value: asset_shortcode(&asset),
                            description: entry.description.clone().unwrap_or_default(),
                            enabled: entry.enabled,
                            kind: KeyValueKind::File,
                            ..KeyValue::default()
                        });
                    }
                } else {
                    out.extend(rows(std::slice::from_ref(entry), ctx));
                }
            }
            Body::FormData(out)
        }
        "graphql" => Body::GraphQL {
            query: ctx.re(doc.text("body:graphql").unwrap_or_default()),
            operation_name: None,
            variables: ctx.re(doc.text("body:graphql:vars").unwrap_or_default()),
            extras: Extras::default(),
        },
        "file" => {
            let asset = doc
                .dict("body:file")
                .unwrap_or_default()
                .iter()
                .filter(|entry| entry.enabled)
                .find_map(|entry| file_refs(&entry.value).and_then(|paths| paths.into_iter().next()));
            match asset {
                Some(asset) => {
                    ctx.report.normal(path, format!("import `{asset}` into Assets"));
                    Body::Binary { path: asset_shortcode(&asset), extras: Extras::default() }
                }
                None => {
                    ctx.report.drop(path, "body mode `file` has no selected file");
                    Body::None
                }
            }
        }
        other => {
            ctx.report.drop(path, format!("body mode `{other}` is not supported"));
            Body::None
        }
    }
}

fn map_assertions(doc: &BruDoc, path: &str, ctx: &mut Ctx) -> Vec<Assertion> {
    let mut out = Vec::new();
    for entry in doc.dict("assert").unwrap_or_default() {
        let text = ctx.re(entry.value.trim());
        let (token, rest) = match text.split_once(char::is_whitespace) {
            Some((token, rest)) => (token, rest.trim()),
            None => (text.as_str(), ""),
        };
        match AssertOperator::from_token(token) {
            Some(operator) => {
                out.push(Assertion { enabled: entry.enabled, expr: entry.key.clone(), operator, value: rest.to_string(), ..Assertion::default() })
            }
            None => {
                ctx.report.normal(
                    path,
                    format!(
                        "assertion `{}` uses unknown operator `{token}`; imported disabled as `equals` with the original text as its value",
                        entry.key
                    ),
                );
                out.push(Assertion {
                    enabled: false,
                    expr: entry.key.clone(),
                    operator: AssertOperator::Eq,
                    value: text.clone(),
                    ..Assertion::default()
                });
            }
        }
    }
    out
}

/// Module names `require("...")` loads that Postguy cannot provide.
fn npm_requires(script: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = script;
    while let Some(at) = rest.find("require(") {
        rest = rest.get(at + "require(".len()..).unwrap_or_default();
        let trimmed = rest.trim_start();
        let Some(quote) = trimmed.chars().next().filter(|c| matches!(c, '\'' | '"' | '`')) else { continue };
        let body = trimmed.get(1..).unwrap_or_default();
        let Some(end) = body.find(quote) else { continue };
        let spec = body.get(..end).unwrap_or_default();
        let bare = spec.strip_prefix("node:").unwrap_or(spec);
        if spec.starts_with('.') || spec.starts_with('/') || matches!(bare, "fs" | "path" | "buffer") || spec.is_empty() {
            continue;
        }
        if !out.iter().any(|seen| seen == spec) {
            out.push(spec.to_string());
        }
    }
    out
}

fn map_request(source: RequestSource, inherited: &[KeyValue], ctx: &mut Ctx) -> Option<Request> {
    let RequestSource { path, doc, name, .. } = source;
    let kind = doc.value("meta", "type").map_or("http", str::trim);
    if !kind.is_empty() && kind != "http" && kind != "graphql" {
        ctx.report.drop(&path, format!("request type `{kind}` is not supported"));
        return None;
    }
    for block in &doc.blocks {
        let tag = block.tag.as_str();
        if matches!(tag, "trace" | "connect") {
            ctx.report.drop(&path, format!("method `{tag}` is not supported"));
            return None;
        }
        if !REQUEST_TAGS.contains(&tag) && !tag.starts_with("auth:") && !tag.starts_with("body:") {
            ctx.report.drop(&format!("{path}: {tag}"), "not supported");
        }
    }
    let methods = [
        ("get", HttpMethod::Get),
        ("post", HttpMethod::Post),
        ("put", HttpMethod::Put),
        ("patch", HttpMethod::Patch),
        ("delete", HttpMethod::Delete),
        ("options", HttpMethod::Options),
        ("head", HttpMethod::Head),
    ];
    let Some((tag, method)) = methods.iter().find(|(tag, _)| doc.block(tag).is_some_and(|b| matches!(b.body, BlockBody::Dict(_)))) else {
        ctx.report.drop(&path, "no HTTP method block");
        return None;
    };
    let url = ctx.re(doc.value(tag, "url").unwrap_or_default());
    let mut http = HttpRequest { method: *method, url: url.clone(), ..HttpRequest::default() };

    let parsed = postguy_urlkit::Url::parse(&url);
    if let Some(query) = &parsed.query {
        for param in postguy_urlkit::params_of_query(&query.0) {
            http.params.push(KeyValue { key: param.key.clone(), value: param.value.clone(), ..KeyValue::default() });
        }
    }
    let disabled: Vec<DictEntry> = doc.dict("params:query").unwrap_or_default().iter().filter(|entry| !entry.enabled).cloned().collect();
    http.params.extend(rows(&disabled, ctx));
    http.path_vars = rows(doc.dict("params:path").unwrap_or_default(), ctx);
    http.headers = rows(doc.dict("headers").unwrap_or_default(), ctx);
    for header in inherited {
        if !http.headers.iter().any(|existing| existing.key.eq_ignore_ascii_case(&header.key)) {
            http.headers.push(header.clone());
        }
    }

    let mut mode = doc.value(tag, "body").map(str::trim).unwrap_or_default().to_string();
    if mode.is_empty() && kind == "graphql" {
        mode = "graphql".to_string();
    }
    http.body = map_body(&mode, &doc, &path, ctx);
    let auth_mode = doc.value(tag, "auth").map(str::trim).unwrap_or_default().to_string();
    http.auth = auth_of(&auth_mode, &doc, &path, ctx);

    let pre = format!("{}{}", pre_request_lines(&doc, ctx), doc.text("script:pre-request").unwrap_or_default());
    let post = format!("{}{}", doc.text("script:post-response").map(|t| format!("{t}\n")).unwrap_or_default(), post_response_lines(&doc, ctx));
    let mut scripts = Scripts::default();
    append_script(&mut scripts.prerequest, pre.trim_end_matches('\n'));
    append_script(&mut scripts.test, post.trim_end_matches('\n'));
    http.scripts = scripts;
    let test_script =
        doc.text("tests").filter(|text| !text.trim().is_empty()).map(|text| Script { text: text.to_string(), extras: Extras::default() });
    http.tests = RequestTests { assertions: map_assertions(&doc, &path, ctx), script: test_script, extras: Extras::default() };

    let mut seen: Vec<String> = Vec::new();
    for script in [http.scripts.prerequest.as_ref(), http.scripts.test.as_ref(), http.tests.script.as_ref()].into_iter().flatten() {
        for spec in npm_requires(&script.text) {
            if !seen.contains(&spec) {
                ctx.report.normal(&path, format!("requires npm package `{spec}`, which Postguy cannot load"));
                seen.push(spec);
            }
        }
    }

    ctx.report.requests += 1;
    let description = doc.text("docs").map(|text| ctx.re(text)).unwrap_or_default();
    Some(Request { id: Id::generate(), name, description, kind: RequestKind::Http(http), examples: Vec::new(), extras: Extras::default() })
}

fn map_environment(name: &str, doc: &BruDoc, ctx: &mut Ctx) -> Environment {
    let mut variables = Vec::new();
    for entry in doc.dict("vars").unwrap_or_default() {
        let value = ctx.re(&entry.value);
        variables.push(Variable { key: entry.key.clone(), value, enabled: entry.enabled, ..Variable::default() });
    }
    for item in doc.list("vars:secret").unwrap_or_default() {
        let (enabled, key) = match item.strip_prefix('~') {
            Some(rest) => (false, rest.trim()),
            None => (true, item.as_str()),
        };
        if !key.is_empty() {
            variables.push(Variable { key: key.to_string(), enabled, secret: true, ..Variable::default() });
        }
    }
    for block in &doc.blocks {
        if !matches!(block.tag.as_str(), "vars" | "vars:secret") {
            ctx.report.drop(&format!("environments/{name}.bru: {}", block.tag), "not supported");
        }
    }
    Environment { id: Id::generate(), name: name.to_string(), color: None, variables, extras: Extras::default() }
}
