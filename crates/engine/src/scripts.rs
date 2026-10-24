//! Script execution glue: collects the scripts around a request, builds the sandbox inputs
//! from the scope chain, runs them off the async workers, applies request edits and persists
//! variable writes. The sandbox itself lives in `postguy-script`.

use std::collections::BTreeMap;
use std::sync::Arc;

use postguy_http::Cancel;
use postguy_model::{
    Body, HttpMethod, HttpRequest, Id, KeyValue, NextRequest, RawLanguage, ScriptFailure, ScriptPhase, Scripts, TestResult, Variable,
};
use postguy_script as js;
use postguy_urlkit::{Param, Url, params_of_query, query_of_params};
use postguy_vars::{Scope, ScopeKind};
use serde::Serialize;

use crate::Engine;
use crate::console::{ConsoleKind, ConsoleLevel};
use crate::error::{EngineError, Result};
use crate::pipeline::{IterationScope, Located, ScopeSet, SendOptions, SendTarget};

/// What the scripts of one send reported.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptReport {
    pub tests: Vec<TestResult>,
    pub failures: Vec<ScriptFailure>,
    pub next_request: NextRequest,
    /// A script wrote an environment, globals or collection variable that is now on disk.
    pub variables_changed: bool,
}

/// The scripts that run in `phase`, outermost first, with the label failures report.
pub(crate) fn sources(located: &Located, phase: ScriptPhase) -> Vec<js::ScriptSource> {
    let pick = |scripts: &Scripts| match phase {
        ScriptPhase::PreRequest => scripts.prerequest.clone(),
        ScriptPhase::PostResponse => scripts.test.clone(),
    };
    let mut out = Vec::new();
    let mut push = |origin: String, scripts: &Scripts| {
        if let Some(script) = pick(scripts).filter(|s| !s.is_empty()) {
            out.push(js::ScriptSource { origin, text: script.text });
        }
    };
    push(format!("collection {}", located.collection.name), &located.collection.scripts);
    for folder in &located.folders {
        push(format!("folder {}", folder.name), &folder.scripts);
    }
    push(format!("request {}", located.request_name), &located.http.scripts);
    out
}

fn first_wins(scope: &Scope) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (key, value) in scope.entries.iter().rev() {
        map.insert(key.clone(), value.clone());
    }
    map
}

/// The script-visible variables from the loaded scope chain.
pub(crate) fn script_variables(set: &ScopeSet, iteration: Option<&IterationScope>, locals: &BTreeMap<String, String>) -> js::Variables {
    let mut vars = js::Variables { locals: locals.clone(), data: iteration.map(|i| i.data.clone()).unwrap_or_default(), ..js::Variables::default() };
    for scope in &set.scopes {
        match scope.kind {
            ScopeKind::Globals => vars.globals = first_wins(scope),
            ScopeKind::Collection => vars.collection = first_wins(scope),
            ScopeKind::Folder => vars.folders.extend(first_wins(scope)),
            ScopeKind::Environment => {
                let name = set.environment.as_ref().map(|(_, name)| name.clone()).unwrap_or_default();
                vars.environment = Some((name, first_wins(scope)));
            }
            ScopeKind::Data | ScopeKind::Local => {}
        }
    }
    vars
}

/// The URL as scripts see it: the typed URL with the enabled query rows merged in, unresolved.
fn merged_url(request: &HttpRequest) -> String {
    let rows: Vec<&KeyValue> = request.params.iter().filter(|row| row.enabled && !row.key.is_empty()).collect();
    if rows.is_empty() {
        return request.url.clone();
    }
    let mut url = Url::parse(&request.url);
    let mut params: Vec<Param> = url.query.as_ref().map(|query| params_of_query(&query.0)).unwrap_or_default();
    for row in rows {
        match params.iter_mut().find(|param| param.key == row.key) {
            Some(existing) => existing.value = row.value.clone(),
            None => params.push(Param { key: row.key.clone(), value: row.value.clone() }),
        }
    }
    url.query = query_of_params(&params);
    url.build()
}

fn method_text(method: HttpMethod) -> &'static str {
    postguy_http::build::method_text(method)
}

fn parse_method(text: &str) -> Option<HttpMethod> {
    Some(match text.to_ascii_uppercase().as_str() {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        "HEAD" => HttpMethod::Head,
        "OPTIONS" => HttpMethod::Options,
        _ => return None,
    })
}

fn text_rows(rows: &[KeyValue]) -> Vec<(String, String)> {
    rows.iter()
        .filter(|row| row.enabled && !row.key.is_empty() && row.kind == postguy_model::KeyValueKind::Text)
        .map(|row| (row.key.clone(), row.value.clone()))
        .collect()
}

/// The body as scripts see it; binary and GraphQL bodies are opaque to them.
pub(crate) fn script_body(body: &Body) -> js::ScriptBody {
    match body {
        Body::None => js::ScriptBody::None,
        Body::Raw { text, .. } => js::ScriptBody::Raw(text.clone()),
        Body::UrlEncoded(rows) => js::ScriptBody::UrlEncoded(text_rows(rows)),
        Body::FormData(rows) => js::ScriptBody::FormData(text_rows(rows)),
        _ => js::ScriptBody::Other,
    }
}

/// The request a pre-request script sees and may edit.
pub(crate) fn script_request(located: &Located) -> js::ScriptRequest {
    let http = &located.http;
    js::ScriptRequest {
        id: located.request_id.clone(),
        name: located.request_name.clone(),
        method: method_text(http.method).to_string(),
        url: merged_url(http),
        headers: http
            .headers
            .iter()
            .filter(|row| !row.key.is_empty())
            .map(|row| js::Header { key: row.key.clone(), value: row.value.clone(), enabled: row.enabled })
            .collect(),
        body: script_body(&http.body),
    }
}

/// Copy the edits a pre-request script made onto the request about to be sent.
pub(crate) fn apply_request(request: &mut HttpRequest, before: &js::ScriptRequest, after: &js::ScriptRequest, origin: &str) -> Result<()> {
    if after.method != before.method {
        request.method = parse_method(&after.method).ok_or_else(|| EngineError::Script {
            phase: ScriptPhase::PreRequest.label().to_string(),
            origin: origin.to_string(),
            message: format!("pm.request.method `{}` is not a valid method", after.method),
            line: None,
        })?;
    }
    if after.url != before.url {
        request.url = after.url.clone();
        request.params.clear();
    }
    if after.headers != before.headers {
        let old = std::mem::take(&mut request.headers);
        let mut unused: Vec<Option<KeyValue>> = old.into_iter().map(Some).collect();
        for header in &after.headers {
            let slot = unused.iter_mut().find(|row| row.as_ref().is_some_and(|row| row.key.eq_ignore_ascii_case(&header.key)));
            let mut row = slot.and_then(Option::take).unwrap_or_default();
            row.key = header.key.clone();
            row.value = header.value.clone();
            row.enabled = header.enabled;
            request.headers.push(row);
        }
    }
    if after.body != before.body
        && let js::ScriptBody::Raw(text) = &after.body
    {
        let language = match &request.body {
            Body::Raw { language, .. } => *language,
            _ => RawLanguage::default(),
        };
        request.body = Body::Raw { language, text: text.clone(), extras: Default::default() };
    }
    Ok(())
}

enum Change {
    Set(String, String),
    Remove(String),
}

fn diff(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<Change> {
    let mut changes: Vec<Change> = before.keys().filter(|key| !after.contains_key(*key)).map(|key| Change::Remove(key.clone())).collect();
    changes.extend(after.iter().filter(|(key, value)| before.get(*key) != Some(*value)).map(|(key, value)| Change::Set(key.clone(), value.clone())));
    changes
}

fn apply_changes(variables: &mut Vec<Variable>, changes: &[Change]) {
    for change in changes {
        match change {
            Change::Remove(key) => variables.retain(|variable| variable.key != *key),
            Change::Set(key, value) => match variables.iter_mut().find(|variable| variable.key == *key) {
                Some(variable) => {
                    variable.value = value.clone();
                    variable.enabled = true;
                }
                None => variables.push(Variable { key: key.clone(), value: value.clone(), ..Variable::default() }),
            },
        }
    }
}

/// Write what a script changed in globals, the active environment and the collection back to
/// disk, re-reading each file first so concurrent edits elsewhere are not clobbered. Locals and
/// folder variables are never written. Returns whether anything was saved.
pub(crate) fn persist(
    engine: &Engine,
    located: &mut Located,
    environment: Option<&(Id, String)>,
    before: &js::Variables,
    after: &js::Variables,
    warnings: &mut Vec<String>,
) -> Result<bool> {
    let mut saved = false;
    let globals = diff(&before.globals, &after.globals);
    if !globals.is_empty() {
        let mut model = engine.workspace().load_globals()?.into_value();
        apply_changes(&mut model.variables, &globals);
        engine.workspace().save_globals(&model)?;
        saved = true;
    }
    if let (Some((id, _)), Some((_, old)), Some((_, new))) = (environment, &before.environment, &after.environment) {
        let changes = diff(old, new);
        if !changes.is_empty() {
            let mut model = engine.workspace().load_environment(id)?.into_value();
            apply_changes(&mut model.variables, &changes);
            engine.workspace().save_environment(&model)?;
            saved = true;
        }
    }
    let collection = diff(&before.collection, &after.collection);
    if !collection.is_empty() {
        if located.collection.id.0.is_empty() {
            warnings.push("a script changed collection variables, but this request is not in a collection, so they were not saved".to_string());
        } else {
            let mut model = engine.workspace().load_collection(&located.collection.id)?.into_value();
            apply_changes(&mut model.variables, &collection);
            engine.workspace().save_collection(&model)?;
            located.collection.variables = model.variables;
            saved = true;
        }
    }
    Ok(saved)
}

/// Sends `pm.sendRequest` through the engine and answers cookie lookups from its jar.
struct EngineHost {
    engine: Engine,
    handle: tokio::runtime::Handle,
    cancel: Cancel,
    environment: Option<Id>,
    iteration: Option<IterationScope>,
}

impl js::Host for EngineHost {
    fn send_request(&self, request: js::ScriptRequest) -> std::result::Result<js::ScriptResponse, String> {
        let method = parse_method(&request.method).ok_or_else(|| format!("`{}` is not a valid method", request.method))?;
        let rows = |pairs: &[(String, String)]| -> Vec<KeyValue> {
            pairs.iter().map(|(key, value)| KeyValue { key: key.clone(), value: value.clone(), ..KeyValue::default() }).collect()
        };
        let body = match &request.body {
            js::ScriptBody::Raw(text) => Body::Raw { language: RawLanguage::Text, text: text.clone(), extras: Default::default() },
            js::ScriptBody::UrlEncoded(pairs) => Body::UrlEncoded(rows(pairs)),
            js::ScriptBody::FormData(pairs) => Body::FormData(rows(pairs)),
            _ => Body::None,
        };
        let http = HttpRequest {
            method,
            url: request.url.clone(),
            headers: request
                .headers
                .iter()
                .filter(|h| h.enabled)
                .map(|h| KeyValue { key: h.key.clone(), value: h.value.clone(), ..KeyValue::default() })
                .collect(),
            body,
            ..HttpRequest::default()
        };
        let options = SendOptions {
            environment: self.environment.clone(),
            cancel: Some(self.cancel.clone()),
            run_scripts: false,
            iteration: self.iteration.clone(),
        };
        let target = SendTarget::Inline { request: Box::new(http), name: "pm.sendRequest".to_string() };
        let outcome = self.handle.block_on(crate::pipeline::execute(&self.engine, target, options)).map_err(|err| err.to_string())?;
        let view = outcome.response;
        Ok(js::ScriptResponse {
            code: view.status,
            status: view.status_text,
            headers: view.headers,
            body: view.body,
            response_time_ms: view.timings.total_ms,
            size: u64::try_from(view.size.body_decoded_bytes).unwrap_or(u64::MAX),
        })
    }

    fn cookies_for(&self, url: &str) -> Vec<(String, String)> {
        let header = self.engine.jar().lock().map(|jar| jar.header_for(url)).unwrap_or_default();
        header.split(';').filter_map(|pair| pair.trim().split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }
}

/// One phase's result, ready for the pipeline.
pub(crate) struct PhaseOutput {
    pub outcome: js::Outcome,
    pub variables_changed: bool,
}

/// Everything a phase needs besides the located request.
pub(crate) struct PhaseInput<'a> {
    pub phase: ScriptPhase,
    pub scopes: &'a ScopeSet,
    pub options: &'a SendOptions,
    pub locals: &'a BTreeMap<String, String>,
    pub request: js::ScriptRequest,
    pub response: Option<js::ScriptResponse>,
}

/// Run the scripts of one phase. Returns `None` when there is nothing to run.
pub(crate) async fn run_phase(
    engine: &Engine,
    located: &mut Located,
    input: PhaseInput<'_>,
    warnings: &mut Vec<String>,
) -> Result<Option<PhaseOutput>> {
    let scripts = sources(located, input.phase);
    if scripts.is_empty() {
        return Ok(None);
    }
    let before = script_variables(input.scopes, input.options.iteration.as_ref(), input.locals);
    let iteration = input.options.iteration.clone().unwrap_or_default();
    let cancel = input.options.cancel.clone().unwrap_or_default();
    let context = js::Context {
        phase: input.phase,
        info: js::Info {
            event_name: if input.phase == ScriptPhase::PreRequest { "prerequest" } else { "test" },
            iteration: iteration.index,
            iteration_count: iteration.count.max(1),
            request_name: located.request_name.clone(),
            request_id: located.request_id.clone(),
        },
        variables: before.clone(),
        request: input.request.clone(),
        response: input.response,
        host: Arc::new(EngineHost {
            engine: engine.clone(),
            handle: tokio::runtime::Handle::current(),
            cancel: cancel.clone(),
            environment: input.options.environment.clone(),
            iteration: input.options.iteration.clone(),
        }),
    };
    let check_cancel = cancel.clone();
    let cancelled: js::CancelCheck = Arc::new(move || check_cancel.is_cancelled());
    let limits = js::Limits::default();
    let origin = scripts.first().map(|s| s.origin.clone()).unwrap_or_default();
    let phase = input.phase;
    let outcome = tokio::task::spawn_blocking(move || js::run(&scripts, context, &limits, cancelled)).await.map_err(|err| EngineError::Script {
        phase: phase.label().to_string(),
        origin,
        message: format!("the script engine failed: {err}"),
        line: None,
    })?;

    let variables_changed = persist(engine, located, input.scopes.environment.as_ref(), &before, &outcome.variables, warnings)?;
    let secret_scopes = &input.scopes.scopes;
    for line in &outcome.logs {
        let level = match line.level {
            js::LogLevel::Warn => ConsoleLevel::Warning,
            js::LogLevel::Error => ConsoleLevel::Error,
            _ => ConsoleLevel::Info,
        };
        let message = format!("[{}] {}", input.phase.label(), line.message);
        engine.console().push(level, ConsoleKind::Log, postguy_vars::mask_secrets(&message, secret_scopes));
    }
    Ok(Some(PhaseOutput { outcome, variables_changed }))
}

/// The scope set with `locals` appended, as a resolver.
pub(crate) fn resolver_with_locals(set: &ScopeSet, locals: &BTreeMap<String, String>) -> postguy_vars::Resolver {
    let mut resolver = postguy_vars::Resolver::new();
    for scope in &set.scopes {
        resolver = resolver.with_scope(scope.clone());
    }
    if !locals.is_empty() {
        resolver = resolver.with_scope(Scope::new(ScopeKind::Local, locals.clone()));
    }
    resolver
}
