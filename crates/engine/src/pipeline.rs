//! The send pipeline (PROMPT.md 5.3.2, steps 1–12) with scripts as explicit no-op hooks until
//! M8: scope chain → script hook → auth inheritance/resolution → URL build → auth application →
//! body/automatic headers → cookies → send → cookie update → response model → post-script hook →
//! history/console record. Each step lives in its own function so it is unit-testable, and the
//! whole run is wrapped in `catch_unwind` by [`crate::Engine::send`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use postguy_http::client::ResponseSize;
use postguy_http::{AuthPlacement, Cancel, RedirectHop, RedirectPolicy, RequestSpec, ResponseData, ResponseTimings};
use postguy_model::{AuthConfig, Body, Collection, Folder, HistoryEntry, HttpRequest, Id, Item, KeyValue, Request, RequestKind, Variable};
use postguy_urlkit::{Param, Url, encode_component, encode_query, host_to_ascii, query_of_params};
use postguy_vars::{Resolver, Scope, ScopeKind};
use serde::Serialize;

use crate::Engine;
use crate::console::{ConsoleKind, ConsoleLevel, truncate_for_log};
use crate::error::{EngineError, Result};

/// Where a send starts.
#[derive(Debug, Clone)]
pub enum SendTarget {
    /// A request saved in a collection (variables, auth and scripts come from its ancestors).
    Stored { collection: Id, request: Id },
    /// A request assembled by a caller (automation, tests): only environment/global scopes.
    Inline { request: Box<HttpRequest>, name: String },
}

/// Knobs for one send.
#[derive(Debug, Clone, Default)]
pub struct SendOptions {
    /// Explicit environment; None uses the workspace's active environment.
    pub environment: Option<Id>,
    /// The caller's cancellation handle (Esc in the UI, `request_cancel` from a tool).
    pub cancel: Option<Cancel>,
    /// Run pre-request and test scripts when the sandbox exists (M8). Today a no-op that warns.
    pub run_scripts: bool,
}

/// The request exactly as it went on the wire (secrets masked in the history snapshot).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SentRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body_len: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthPlacement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    pub redirect_hops: Vec<RedirectHop>,
}

/// What the response viewer and the `response_*` tools read.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseView {
    pub status: u16,
    pub status_text: String,
    pub http_version: String,
    pub headers: Vec<(String, String)>,
    /// DECODED bytes (compression undone when enabled).
    pub body: Vec<u8>,
    pub size: ResponseSize,
    pub timings: ResponseTimings,
}

/// One completed send.
#[derive(Debug, Clone)]
pub struct SendOutcome {
    pub request: SentRequest,
    pub response: ResponseView,
    pub history: HistoryEntry,
    pub warnings: Vec<String>,
}

/// The request plus everything that supplies scopes, auth and scripts around it.
struct Located {
    collection: Collection,
    folders: Vec<Folder>,
    http: HttpRequest,
}

/// Run one send (async core; [`Engine::send`] adds panic protection and a blocking facade).
pub(crate) async fn execute(engine: &Engine, target: SendTarget, options: SendOptions) -> Result<SendOutcome> {
    let global = engine.settings();
    let mut warnings: Vec<String> = Vec::new();

    // 1. Scope chain and request location.
    let located = locate(engine, &target)?;
    let resolver = build_resolver(engine, &located, options.environment.as_ref())?;

    // 2. Pre-request scripts (collection, folders outer→inner, request): the explicit no-op.
    let scripts_present = scripts_present(&located);
    if scripts_present {
        warnings.push("scripts are saved but not executed in this build (scripting arrives in M8)".to_string());
        engine.console().push(ConsoleLevel::Warning, ConsoleKind::Warning, "scripts were skipped: the script sandbox is not built yet (M8)");
    }
    let _ = options.run_scripts;

    // 3. Auth inheritance and variable resolution.
    let auth = resolve_auth(&resolved_auth_chain(&located), &resolver)?;

    // 4. URL: resolve, path vars, enabled query params, encoding.
    let mut request = located.http.clone();
    let mut url = build_url(&request, &resolver, request.settings.encode_url_automatically)?;

    // 5. Auth application (may add a header or rewrite the query).
    request.headers = resolve_rows(&request.headers, &resolver);
    let mut auth_placement = None;
    if let Some(auth) = &auth {
        let mut probe = RequestSpec::get(url.clone());
        auth_placement = Some(postguy_http::apply_auth(auth, &mut probe)?);
        url = probe.url;
        for (name, value) in probe.headers {
            if !request.headers.iter().any(|row| row.key.eq_ignore_ascii_case(&name)) {
                request.headers.push(KeyValue { key: name, value, ..KeyValue::default() });
            }
        }
    }

    // 6. Body and automatic headers.
    request.body = resolve_body(&request.body, &resolver);
    let load_file = file_loader(engine.store_paths(), &global.working_directory, global.allow_files_outside_working_directory);
    let built = postguy_http::build::build(postguy_http::BuildInput {
        method: postguy_http::build::method_text(request.method),
        url: &url,
        headers: &request.headers,
        body: &request.body,
        load_file: Some(&load_file),
        send_no_cache: global.send_no_cache_header,
    })?;
    warnings.extend(built.warnings.iter().cloned());

    // 7. Cookies are merged inside `send_with_cookies` (jar first, then a manual Cookie row).

    // 8. Send: proxy aware, manual redirects, cookies on every hop.
    let client = engine.client_for(&request.settings, &global)?;
    let spec = RequestSpec {
        method: hyper::Method::from_bytes(built.method.as_bytes()).unwrap_or(hyper::Method::GET),
        url: built.url.clone(),
        headers: built.headers.clone(),
        body: built.body.clone(),
        timeout_ms: request.settings.timeout_ms,
        max_body_bytes: max_body_bytes(request.settings.max_response_mb),
        decode_compression: true,
    };
    let policy = RedirectPolicy {
        follow: request.settings.follow_redirects,
        follow_original_method: request.settings.follow_original_method,
        follow_authorization_on_redirect: request.settings.follow_authorization_on_redirect,
        remove_referer_on_redirect: request.settings.remove_referer_on_redirect,
        max_redirects: request.settings.max_redirects,
    };
    let cancel = options.cancel.clone().unwrap_or_default();
    let outcome = {
        let jar = engine.jar();
        client.send_with_cookies(spec, policy, jar, request.settings.cookie_jar_disabled, &cancel).await?
    };

    // 9. Persist the jar (atomic; a failure is a warning, never a lost response).
    if let Err(err) = engine.save_jar() {
        warnings.push(format!("could not save the cookie jar: {err}"));
    }

    // 10. Response model.
    let response = response_view(&outcome.response);
    let status = response.status;
    let duration_ms = response.timings.total_ms;
    let size_bytes = u64::try_from(response.size.body_decoded_bytes).unwrap_or(u64::MAX);
    let proxy = client.proxy_route_for(&built.url).map(|route| route.address());

    // 11. Post-response scripts/tests: the same explicit no-op hook (the warning above covers it).

    // 12. Record: masked history snapshot, Console entries, optional saved body.
    let sent_http = HttpRequest {
        method: request.method,
        url: postguy_vars::mask_secrets(&built.url, resolver.scopes()),
        params: request.params,
        path_vars: request.path_vars,
        headers: mask_rows(&request.headers, resolver.scopes()),
        body: request.body,
        auth: request.auth,
        scripts: request.scripts,
        settings: request.settings,
        extras: Default::default(),
    };
    let mut history = HistoryEntry {
        id: postguy_model::Id::generate(),
        time_ms: postguy_util::time::now_unix_ms(),
        request: Some(RequestKind::Http(sent_http)),
        status: Some(status),
        duration_ms: Some(duration_ms),
        size_bytes: Some(size_bytes),
        response_ref: None,
        extras: serde_json::Map::new(),
    };
    if global.save_history_responses {
        match save_response(engine.store_paths(), &history.id, &response.body) {
            Ok(reference) => history.response_ref = Some(reference),
            Err(err) => warnings.push(format!("could not save the response body: {err}")),
        }
    }
    if let Err(err) = engine.workspace().append_history(&history) {
        warnings.push(format!("could not record history: {err}"));
    }

    let masked_url = postguy_vars::mask_secrets(&url, resolver.scopes());
    let summary = format!(
        "{} {} -> {} {} · {} ms · {} B{}",
        built.method,
        masked_url,
        status,
        response.status_text,
        duration_ms,
        size_bytes,
        proxy.as_ref().map(|address| format!(" · proxy {address}")).unwrap_or_default()
    );
    let detail = format!(
        "request headers:\n{}\n\nresponse headers:\n{}\n\nresponse body:\n{}",
        format_headers(&mask_pairs(&built.headers, resolver.scopes())),
        format_headers(&response.headers),
        truncate_for_log(&response.body)
    );
    engine.console().push_with_detail(
        if status >= 400 { ConsoleLevel::Warning } else { ConsoleLevel::Info },
        ConsoleKind::Request,
        summary,
        Some(postguy_vars::mask_secrets(&detail, resolver.scopes())),
    );
    for warning in &warnings {
        engine.console().push(ConsoleLevel::Warning, ConsoleKind::Warning, postguy_vars::mask_secrets(warning, resolver.scopes()));
    }

    let sent = SentRequest {
        method: built.method,
        url: masked_url,
        headers: mask_pairs(&built.headers, resolver.scopes()),
        body_len: built.body.len(),
        auth: auth_placement,
        proxy,
        redirect_hops: outcome.hops,
    };
    Ok(SendOutcome { request: sent, response, history, warnings })
}

fn scripts_present(located: &Located) -> bool {
    !located.collection.scripts.is_empty() || located.folders.iter().any(|folder| !folder.scripts.is_empty()) || !located.http.scripts.is_empty()
}

fn locate(engine: &Engine, target: &SendTarget) -> Result<Located> {
    match target {
        SendTarget::Inline { request, name } => {
            let _ = name;
            Ok(Located { collection: Collection::default(), folders: Vec::new(), http: (**request).clone() })
        }
        SendTarget::Stored { collection, request } => {
            let collection = engine.workspace().load_collection(collection)?.into_value();
            let mut stack: Vec<(Vec<Folder>, Vec<Item>)> = vec![(Vec::new(), collection.items.clone())];
            let mut visited = 0usize;
            while let Some((folders, items)) = stack.pop() {
                visited = visited.saturating_add(items.len());
                if visited > 200_000 {
                    return Err(EngineError::BadRequest { reason: "the collection tree is too large to search".into() });
                }
                for item in items {
                    match item {
                        Item::Folder(folder) => {
                            let mut child = folders.clone();
                            child.push((*folder).clone());
                            stack.push((child, folder.items.clone()));
                        }
                        Item::Request(found) if found.id == *request => {
                            let Some(http) = http_of(&found).cloned() else {
                                return Err(EngineError::Unsupported { what: format!("{} requests", found.kind.protocol()) });
                            };
                            return Ok(Located { collection, folders, http });
                        }
                        Item::Request(_) => {}
                    }
                }
            }
            Err(EngineError::NotFound { what: "request".into(), id: request.0.clone() })
        }
    }
}

/// The HTTP payload when this is an HTTP request.
fn http_of(request: &Request) -> Option<&HttpRequest> {
    match &request.kind {
        RequestKind::Http(http) => Some(http),
        _ => None,
    }
}

/// Scopes: globals < collection < folders outer→inner < environment (PROMPT.md 5.2). Secret
/// variable values come from the secrets store, never from the model files.
fn build_resolver(engine: &Engine, located: &Located, environment: Option<&Id>) -> Result<Resolver> {
    let mut resolver = Resolver::new();

    let globals = engine.workspace().load_globals()?.into_value();
    let global_secrets = engine.workspace().secrets.values_for("globals").unwrap_or_default();
    resolver = resolver.with_scope(scope_from_variables(ScopeKind::Globals, &globals.variables, &global_secrets));

    let collection_secrets = engine.workspace().secrets.values_for(&located.collection.id.0).unwrap_or_default();
    resolver = resolver.with_scope(scope_from_variables(ScopeKind::Collection, &located.collection.variables, &collection_secrets));
    for folder in &located.folders {
        let folder_secrets = engine.workspace().secrets.values_for(&folder.id.0).unwrap_or_default();
        resolver = resolver.with_scope(scope_from_variables(ScopeKind::Folder, &folder.variables, &folder_secrets));
    }

    let environment_id = environment.cloned().or_else(|| engine.workspace().active_environment_id().ok().flatten());
    if let Some(id) = environment_id {
        let environment = engine.workspace().load_environment(&id)?.into_value();
        let secrets = engine.workspace().secrets.values_for(&id.0).unwrap_or_default();
        resolver = resolver.with_scope(scope_from_variables(ScopeKind::Environment, &environment.variables, &secrets));
    }
    Ok(resolver)
}

fn scope_from_variables(kind: ScopeKind, variables: &[Variable], secrets: &BTreeMap<String, String>) -> Scope {
    let mut entries: Vec<(String, String)> = variables
        .iter()
        .filter(|variable| variable.enabled && !variable.key.is_empty())
        .map(|variable| {
            let value = if variable.secret { secrets.get(&variable.key).cloned().unwrap_or_default() } else { variable.value.clone() };
            (variable.key.clone(), value)
        })
        .collect();
    for (key, value) in secrets {
        if !entries.iter().any(|(existing, _)| existing == key) {
            entries.push((key.clone(), value.clone()));
        }
    }
    let secret_names: Vec<String> = variables.iter().filter(|variable| variable.secret).map(|variable| variable.key.clone()).collect();
    Scope { kind, entries, secrets: secret_names }
}

/// The auth that wins: request, then folders inner→outer, then the collection.
fn resolved_auth_chain(located: &Located) -> Option<AuthConfig> {
    let candidates = std::iter::once(Some(&located.http.auth))
        .chain(located.folders.iter().rev().map(|folder| folder.auth.as_ref()))
        .chain(std::iter::once(located.collection.auth.as_ref()));
    for candidate in candidates {
        match candidate {
            Some(AuthConfig::Inherit) | None => continue,
            Some(auth) => return Some(auth.clone()),
        }
    }
    None
}

/// Resolve `{{vars}}` in the auth fields this build applies; other kinds are cloned so
/// [`postguy_http::apply_auth`] reports them honestly.
fn resolve_auth(auth: &Option<AuthConfig>, resolver: &Resolver) -> Result<Option<AuthConfig>> {
    let Some(auth) = auth else { return Ok(None) };
    let resolved = match auth.clone() {
        AuthConfig::Basic(mut basic) => {
            basic.username = resolver.resolve_text(&basic.username).text;
            basic.password = resolver.resolve_text(&basic.password).text;
            AuthConfig::Basic(basic)
        }
        AuthConfig::Bearer(mut bearer) => {
            bearer.token = resolver.resolve_text(&bearer.token).text;
            bearer.prefix = bearer.prefix.map(|prefix| resolver.resolve_text(&prefix).text);
            AuthConfig::Bearer(bearer)
        }
        AuthConfig::ApiKey(mut api_key) => {
            api_key.key = resolver.resolve_text(&api_key.key).text;
            api_key.value = resolver.resolve_text(&api_key.value).text;
            AuthConfig::ApiKey(api_key)
        }
        other => other,
    };
    Ok(Some(resolved))
}

/// Step 4: resolve the URL, substitute `:pathVars`, merge enabled query params, encode.
fn build_url(request: &HttpRequest, resolver: &Resolver, encode_automatically: bool) -> Result<String> {
    let resolved = resolver.resolve_text(&request.url).text;
    let mut parsed = Url::parse(&resolved);
    if parsed.scheme.is_none() {
        parsed = Url::parse(&format!("http://{resolved}"));
    }
    if parsed.authority.is_empty() {
        return Err(EngineError::BadRequest { reason: format!("the URL `{resolved}` has no host") });
    }
    if parsed.authority.contains("{{") {
        return Err(EngineError::BadRequest {
            reason: format!("the URL `{resolved}` has an unresolved variable in its host; check the variable name and the active environment"),
        });
    }
    parsed.authority = ascii_authority(&parsed.authority)?;
    if !request.path_vars.is_empty() {
        parsed.path = substitute_path_vars(&parsed.path, &request.path_vars, resolver, encode_automatically);
    }
    let params: Vec<&KeyValue> = request.params.iter().filter(|row| row.enabled && !row.key.is_empty()).collect();
    if !params.is_empty() {
        // The URL string stays the source of truth: enabled table rows replace a same-key
        // pair and append the rest, leaving URL-only pairs alone (PROMPT.md 5.3.2).
        let mut rows: Vec<Param> = parsed.query.as_ref().map(|query| postguy_urlkit::params_of_query(&query.0)).unwrap_or_default();
        for row in params {
            let key = resolver.resolve_text(&row.key).text;
            let value = resolver.resolve_text(&row.value).text;
            let key = if encode_automatically { encode_query(&key) } else { key };
            let value = if encode_automatically { encode_query(&value) } else { value };
            if let Some(existing) = rows.iter_mut().find(|param| param.key == key) {
                existing.value = value;
            } else {
                rows.push(Param { key, value });
            }
        }
        parsed.query = query_of_params(&rows);
    }
    Ok(parsed.build())
}

/// IDNA the host while keeping userinfo, port and IPv6 brackets.
fn ascii_authority(authority: &str) -> Result<String> {
    let (userinfo, host_port) = match authority.rsplit_once('@') {
        Some((userinfo, host_port)) => (Some(userinfo), host_port),
        None => (None, authority),
    };
    let converted = if host_port.starts_with('[') {
        host_port.to_string()
    } else if let Some((host, port)) = host_port.rsplit_once(':') {
        if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) {
            format!("{}:{port}", host_to_ascii(host).map_err(|reason| EngineError::BadRequest { reason })?)
        } else {
            host_port.to_string()
        }
    } else {
        host_to_ascii(host_port).map_err(|reason| EngineError::BadRequest { reason })?
    };
    Ok(match userinfo {
        Some(userinfo) => format!("{userinfo}@{converted}"),
        None => converted,
    })
}

fn substitute_path_vars(path: &str, rows: &[KeyValue], resolver: &Resolver, encode: bool) -> String {
    let values: BTreeMap<String, String> =
        rows.iter().filter(|row| row.enabled).map(|row| (row.key.clone(), resolver.resolve_text(&row.value).text)).collect();
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(colon) = rest.find(':') {
        out.push_str(rest.get(..colon).unwrap_or_default());
        let after = rest.get(colon + 1..).unwrap_or_default();
        let name_len = after.bytes().take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_').count();
        let name = after.get(..name_len).unwrap_or_default();
        match values.get(name) {
            Some(value) if !value.is_empty() => {
                if encode {
                    out.push_str(&encode_component(value, br#"/?# "{}"#));
                } else {
                    out.push_str(value);
                }
                rest = after.get(name_len..).unwrap_or_default();
            }
            _ => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Resolve `{{vars}}` in enabled rows (disabled rows are dropped from the wire).
fn resolve_rows(rows: &[KeyValue], resolver: &Resolver) -> Vec<KeyValue> {
    rows.iter()
        .filter(|row| row.enabled && !row.key.trim().is_empty())
        .map(|row| KeyValue {
            key: resolver.resolve_text(&row.key).text,
            value: resolver.resolve_text(&row.value).text,
            description: row.description.clone(),
            enabled: true,
            kind: row.kind,
            content_type: row.content_type.clone(),
            secret: row.secret,
            extras: row.extras.clone(),
        })
        .collect()
}

fn resolve_body(body: &Body, resolver: &Resolver) -> Body {
    match body.clone() {
        Body::None => Body::None,
        Body::FormData(rows) => Body::FormData(resolve_rows(&rows, resolver)),
        Body::UrlEncoded(rows) => Body::UrlEncoded(resolve_rows(&rows, resolver)),
        Body::Raw { language, text, extras } => Body::Raw { language, text: resolver.resolve_text(&text).text, extras },
        Body::Binary { path, extras } => Body::Binary { path: resolver.resolve_text(&path).text, extras },
        Body::GraphQL { query, operation_name, variables, extras } => Body::GraphQL {
            query: resolver.resolve_text(&query).text,
            operation_name: operation_name.map(|name| resolver.resolve_text(&name).text),
            variables: resolver.resolve_text(&variables).text,
            extras,
        },
        other => other,
    }
}

/// The file loader for binary and form-data rows: working-directory confinement (unless the
/// setting allows outside paths) and a 100 MiB cap (PROMPT.md 5.3.2, AGENTS.md 4).
fn file_loader<'a>(
    paths: &'a postguy_store::StorePaths,
    working_directory: &str,
    allow_outside: bool,
) -> impl Fn(&str) -> std::result::Result<Vec<u8>, String> + 'a {
    const MAX_FILE_BYTES: u64 = 100 << 20;
    let base = if working_directory.trim().is_empty() { paths.files_dir() } else { PathBuf::from(working_directory) };
    move |raw: &str| {
        if raw.trim().is_empty() {
            return Err("a file row has no path".to_string());
        }
        let candidate = if Path::new(raw).is_absolute() { PathBuf::from(raw) } else { base.join(raw) };
        let canonical = candidate.canonicalize().map_err(|err| format!("`{raw}` cannot be read: {err}"))?;
        if !allow_outside {
            let root = base.canonicalize().unwrap_or_else(|_| base.clone());
            if !canonical.starts_with(&root) {
                return Err(format!(
                    "`{raw}` is outside the working directory `{}`; enable \"Allow files outside the working directory\" to send it",
                    root.display()
                ));
            }
        }
        let metadata = std::fs::metadata(&canonical).map_err(|err| format!("`{raw}` cannot be read: {err}"))?;
        if metadata.len() > MAX_FILE_BYTES {
            return Err(format!("`{raw}` is larger than the 100 MiB send cap"));
        }
        std::fs::read(&canonical).map_err(|err| format!("`{raw}` cannot be read: {err}"))
    }
}

fn max_body_bytes(max_response_mb: u64) -> usize {
    if max_response_mb == 0 { usize::MAX } else { usize::try_from(max_response_mb.saturating_mul(1 << 20)).unwrap_or(usize::MAX) }
}

fn response_view(response: &ResponseData) -> ResponseView {
    ResponseView {
        status: response.status.as_u16(),
        status_text: response.status.canonical_reason().unwrap_or_default().to_string(),
        http_version: match response.version {
            hyper::Version::HTTP_09 => "HTTP/0.9",
            hyper::Version::HTTP_10 => "HTTP/1.0",
            hyper::Version::HTTP_11 => "HTTP/1.1",
            hyper::Version::HTTP_2 => "HTTP/2",
            hyper::Version::HTTP_3 => "HTTP/3",
            _ => "HTTP/?",
        }
        .to_string(),
        headers: response.headers.clone(),
        body: response.body.clone(),
        size: response.size,
        timings: response.timings,
    }
}

fn save_response(paths: &postguy_store::StorePaths, id: &Id, body: &[u8]) -> std::result::Result<String, String> {
    let directory = paths.responses_dir();
    std::fs::create_dir_all(&directory).map_err(|err| format!("creating {}: {err}", directory.display()))?;
    let reference = format!("{}.bin", id.0);
    postguy_util::atomic_write(&directory.join(&reference), body).map_err(|err| err.to_string())?;
    Ok(reference)
}

fn format_headers(headers: &[(String, String)]) -> String {
    headers.iter().map(|(name, value)| format!("{name}: {value}")).collect::<Vec<String>>().join("\n")
}

fn mask_pairs(pairs: &[(String, String)], scopes: &[Scope]) -> Vec<(String, String)> {
    pairs.iter().map(|(name, value)| (name.clone(), postguy_vars::mask_secrets(value, scopes))).collect()
}

fn mask_rows(rows: &[KeyValue], scopes: &[Scope]) -> Vec<KeyValue> {
    rows.iter()
        .map(|row| KeyValue {
            key: row.key.clone(),
            value: postguy_vars::mask_secrets(&row.value, scopes),
            description: row.description.clone(),
            enabled: row.enabled,
            kind: row.kind,
            content_type: row.content_type.clone(),
            secret: row.secret,
            extras: row.extras.clone(),
        })
        .collect()
}
