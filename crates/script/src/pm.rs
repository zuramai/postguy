//! The `pm`, `postman` and `console` globals plus `atob`/`btoa`/`setTimeout`/`require`, all
//! as Rust closures over one shared [`State`]. No JS prelude.
//!
//! Closures never keep JS values alive across calls (a Rust-held JS value inside a JS
//! function is a reference cycle QuickJS cannot collect); everything JS-shaped is rebuilt per
//! call or stored on the JS side.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use postguy_model::{NextRequest, ScriptFailure, ScriptPhase, TestResult, TestStatus};
use postguy_urlkit::{Param, Url, params_of_query, query_of_params};
use postguy_vars::{Resolver, Scope as VarScope, ScopeKind};
use rquickjs::function::{IntoJsFunc, Opt, Rest, This};
use rquickjs::object::Accessor;
use rquickjs::{Array, Ctx, Exception, Function, Object, Result, Value};

use crate::convert::{self, Js};
use crate::expect;
use crate::{CancelCheck, Header, Host, Info, Limits, LogLevel, LogLine, ScriptBody, ScriptRequest, ScriptResponse, Variables};

/// Longest console line kept.
const LOG_LINE_CAP: usize = 16 * 1024;
/// Longest variable value a script may store.
const VALUE_CAP: usize = 1 << 20;
/// Most queued `setTimeout` callbacks.
pub(crate) const MAX_TIMERS: usize = 10_000;
/// The JS-side queue of `setTimeout` entries.
pub(crate) const TIMERS: &str = "__pgTimers";

pub(crate) struct State {
    pub phase: ScriptPhase,
    pub info: Info,
    pub vars: Variables,
    pub request: ScriptRequest,
    pub response: Option<ScriptResponse>,
    pub host: Arc<dyn Host>,
    pub limits: Limits,
    pub cancelled: CancelCheck,
    pub tests: Vec<TestResult>,
    pub failures: Vec<ScriptFailure>,
    pub logs: Vec<LogLine>,
    pub sends: u32,
    pub next: NextRequest,
    pub skip: bool,
    pub tests_overflowed: bool,
    pub origin: String,
}

pub(crate) type Shared = Rc<RefCell<State>>;

pub(crate) fn throw<'js>(ctx: &Ctx<'js>, message: &str) -> rquickjs::Error {
    Exception::throw_message(ctx, message)
}

/// Run `f` on the state. Never panics: a re-entrant borrow is a JS error.
pub(crate) fn with_state<'js, R>(ctx: &Ctx<'js>, shared: &Shared, f: impl FnOnce(&mut State) -> R) -> Result<R> {
    match shared.try_borrow_mut() {
        Ok(mut state) => Ok(f(&mut state)),
        Err(_) => Err(throw(ctx, "internal error: the script state is busy")),
    }
}

fn def<'js, P, F>(ctx: &Ctx<'js>, object: &Object<'js>, name: &str, f: F) -> Result<()>
where
    F: IntoJsFunc<'js, P> + 'js,
{
    let function = Function::new(ctx.clone(), f)?.with_name(name)?;
    object.set(name, function)
}

/// `value` as variable text: strings as-is, everything else through `JSON.stringify`.
fn var_text<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
    let text = if let Some(s) = value.as_string() {
        s.to_string()?
    } else if value.is_undefined() {
        "undefined".to_string()
    } else {
        match ctx.json_stringify(value.clone())? {
            Some(s) => s.to_string()?,
            None => "undefined".to_string(),
        }
    };
    if text.len() > VALUE_CAP {
        return Err(throw(ctx, "a variable value may not be larger than 1 MiB"));
    }
    Ok(text)
}

/// Any value as display text (`String(value)` for primitives, JSON for objects).
fn display_text<'js>(value: &Value<'js>) -> String {
    if let Some(s) = value.as_string() {
        return s.to_string().unwrap_or_default();
    }
    match convert::from_value(value) {
        Ok(js) => match js {
            Js::Str(s) => s,
            other => convert::inspect(&other),
        },
        Err(_) => "[value too large]".to_string(),
    }
}

/// The `message` of a caught value.
pub(crate) fn error_message<'js>(value: &Value<'js>) -> String {
    if let Some(object) = value.as_object()
        && let Ok(message) = object.get::<_, String>("message")
    {
        return message;
    }
    display_text(value)
}

// ----------------------------------------------------------------------------------------
// Variable scopes

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Globals,
    Collection,
    Environment,
    Variables,
    Data,
}

fn merged(vars: &Variables) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut put = |key: &str, value: String| {
        if let Some(slot) = out.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value;
        } else {
            out.push((key.to_string(), value));
        }
    };
    for (k, v) in &vars.globals {
        put(k, v.clone());
    }
    for (k, v) in &vars.collection {
        put(k, v.clone());
    }
    for (k, v) in &vars.folders {
        put(k, v.clone());
    }
    if let Some((_, env)) = &vars.environment {
        for (k, v) in env {
            put(k, v.clone());
        }
    }
    for (k, v) in &vars.data {
        put(k, json_text(v));
    }
    for (k, v) in &vars.locals {
        put(k, v.clone());
    }
    out
}

fn json_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn get_var<'js>(ctx: &Ctx<'js>, shared: &Shared, scope: Scope, key: &str) -> Result<Value<'js>> {
    enum Found {
        Text(String),
        Json(serde_json::Value),
        Missing,
    }
    let found = with_state(ctx, shared, |s| {
        let v = &s.vars;
        match scope {
            Scope::Globals => v.globals.get(key).cloned().map_or(Found::Missing, Found::Text),
            Scope::Collection => v.collection.get(key).cloned().map_or(Found::Missing, Found::Text),
            Scope::Environment => v.environment.as_ref().and_then(|(_, m)| m.get(key)).cloned().map_or(Found::Missing, Found::Text),
            Scope::Data => v.data.iter().find(|(k, _)| k == key).map_or(Found::Missing, |(_, j)| Found::Json(j.clone())),
            Scope::Variables => {
                if let Some(text) = v.locals.get(key) {
                    return Found::Text(text.clone());
                }
                if let Some((_, json)) = v.data.iter().find(|(k, _)| k == key) {
                    return Found::Json(json.clone());
                }
                let env = v.environment.as_ref().and_then(|(_, m)| m.get(key));
                env.or_else(|| v.folders.get(key))
                    .or_else(|| v.collection.get(key))
                    .or_else(|| v.globals.get(key))
                    .cloned()
                    .map_or(Found::Missing, Found::Text)
            }
        }
    })?;
    match found {
        Found::Text(text) => Ok(rquickjs::String::from_str(ctx.clone(), &text)?.into_value()),
        Found::Json(json) => convert::json_to_js(ctx, &json),
        Found::Missing => Ok(Value::new_undefined(ctx.clone())),
    }
}

fn writable_map(vars: &mut Variables, scope: Scope) -> std::result::Result<&mut std::collections::BTreeMap<String, String>, &'static str> {
    match scope {
        Scope::Globals => Ok(&mut vars.globals),
        Scope::Collection => Ok(&mut vars.collection),
        Scope::Variables => Ok(&mut vars.locals),
        Scope::Environment => match vars.environment.as_mut() {
            Some((_, values)) => Ok(values),
            None => Err("no environment is selected; choose one before setting environment variables"),
        },
        Scope::Data => Err("pm.iterationData is read-only"),
    }
}

fn scope_object<'js>(ctx: &Ctx<'js>, shared: &Shared, scope: Scope) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    {
        let shared = shared.clone();
        def(ctx, &object, "get", move |ctx: Ctx<'js>, key: String| get_var(&ctx, &shared, scope, &key))?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "has", move |ctx: Ctx<'js>, key: String| -> Result<bool> { Ok(!get_var(&ctx, &shared, scope, &key)?.is_undefined()) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "toObject", move |ctx: Ctx<'js>| -> Result<Object<'js>> {
            let pairs = with_state(&ctx, &shared, |s| match scope {
                Scope::Globals => s.vars.globals.clone().into_iter().collect::<Vec<_>>(),
                Scope::Collection => s.vars.collection.clone().into_iter().collect(),
                Scope::Environment => s.vars.environment.as_ref().map(|(_, m)| m.clone().into_iter().collect()).unwrap_or_default(),
                Scope::Variables => merged(&s.vars),
                Scope::Data => s.vars.data.iter().map(|(k, v)| (k.clone(), json_text(v))).collect(),
            })?;
            if scope == Scope::Data {
                let object = Object::new(ctx.clone())?;
                let data = with_state(&ctx, &shared, |s| s.vars.data.clone())?;
                for (key, value) in data {
                    object.set(key, convert::json_to_js(&ctx, &value)?)?;
                }
                return Ok(object);
            }
            convert::pairs_to_object(&ctx, pairs)
        })?;
    }
    if scope == Scope::Data {
        return Ok(object);
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "set", move |ctx: Ctx<'js>, key: String, value: Value<'js>| -> Result<()> {
            let text = var_text(&ctx, &value)?;
            let outcome = with_state(&ctx, &shared, |s| writable_map(&mut s.vars, scope).map(|map| map.insert(key, text)).map(|_| ()))?;
            outcome.map_err(|message| throw(&ctx, message))
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "unset", move |ctx: Ctx<'js>, key: String| -> Result<()> {
            let outcome = with_state(&ctx, &shared, |s| writable_map(&mut s.vars, scope).map(|map| map.remove(&key)).map(|_| ()))?;
            outcome.map_err(|message| throw(&ctx, message))
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "clear", move |ctx: Ctx<'js>| -> Result<()> {
            let outcome = with_state(&ctx, &shared, |s| writable_map(&mut s.vars, scope).map(|map| map.clear()))?;
            outcome.map_err(|message| throw(&ctx, message))
        })?;
    }
    if scope == Scope::Variables {
        let shared = shared.clone();
        def(ctx, &object, "replaceIn", move |ctx: Ctx<'js>, text: String| -> Result<String> {
            with_state(&ctx, &shared, |s| {
                let v = &s.vars;
                let to_scope = |kind, map: &std::collections::BTreeMap<String, String>| VarScope::new(kind, map.clone());
                let mut resolver = Resolver::new()
                    .with_scope(to_scope(ScopeKind::Globals, &v.globals))
                    .with_scope(to_scope(ScopeKind::Collection, &v.collection))
                    .with_scope(to_scope(ScopeKind::Folder, &v.folders));
                if let Some((_, env)) = &v.environment {
                    resolver = resolver.with_scope(to_scope(ScopeKind::Environment, env));
                }
                resolver = resolver.with_scope(VarScope::new(ScopeKind::Data, v.data.iter().map(|(k, j)| (k.clone(), json_text(j)))));
                resolver = resolver.with_scope(to_scope(ScopeKind::Local, &v.locals));
                resolver.resolve_text(&text).text
            })
        })?;
    }
    if scope == Scope::Environment {
        let shared = shared.clone();
        object.prop(
            "name",
            Accessor::from(move |ctx: Ctx<'js>| -> Result<Option<String>> {
                with_state(&ctx, &shared, |s| s.vars.environment.as_ref().map(|(name, _)| name.clone()))
            })
            .enumerable(),
        )?;
    }
    Ok(object)
}

// ----------------------------------------------------------------------------------------
// pm.request

fn ensure_writable<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<()> {
    if with_state(ctx, shared, |s| s.phase == ScriptPhase::PostResponse)? {
        return Err(throw(ctx, "pm.request is read-only in post-response scripts"));
    }
    Ok(())
}

fn key_value<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<(String, String)> {
    let key: Value = object.get("key")?;
    let value: Value = object.get("value")?;
    if key.is_undefined() {
        return Err(throw(ctx, "expected an object with a key and a value"));
    }
    Ok((display_text(&key), if value.is_undefined() { String::new() } else { display_text(&value) }))
}

fn headers_object<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    {
        let shared = shared.clone();
        def(ctx, &object, "get", move |ctx: Ctx<'js>, key: String| -> Result<Option<String>> {
            with_state(&ctx, &shared, |s| s.request.headers.iter().find(|h| h.enabled && h.key.eq_ignore_ascii_case(&key)).map(|h| h.value.clone()))
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "has", move |ctx: Ctx<'js>, key: String| -> Result<bool> {
            with_state(&ctx, &shared, |s| s.request.headers.iter().any(|h| h.enabled && h.key.eq_ignore_ascii_case(&key)))
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "toObject", move |ctx: Ctx<'js>| -> Result<Object<'js>> {
            let pairs = with_state(&ctx, &shared, |s| {
                s.request.headers.iter().filter(|h| h.enabled).map(|h| (h.key.clone(), h.value.clone())).collect::<Vec<_>>()
            })?;
            convert::pairs_to_object(&ctx, pairs)
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "add", move |ctx: Ctx<'js>, item: Object<'js>| -> Result<()> {
            ensure_writable(&ctx, &shared)?;
            let (key, value) = key_value(&ctx, &item)?;
            with_state(&ctx, &shared, |s| s.request.headers.push(Header { key, value, enabled: true }))
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "upsert", move |ctx: Ctx<'js>, item: Object<'js>| -> Result<()> {
            ensure_writable(&ctx, &shared)?;
            let (key, value) = key_value(&ctx, &item)?;
            with_state(&ctx, &shared, |s| match s.request.headers.iter_mut().find(|h| h.key.eq_ignore_ascii_case(&key)) {
                Some(header) => {
                    header.value = value;
                    header.enabled = true;
                }
                None => s.request.headers.push(Header { key, value, enabled: true }),
            })
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "remove", move |ctx: Ctx<'js>, key: String| -> Result<()> {
            ensure_writable(&ctx, &shared)?;
            with_state(&ctx, &shared, |s| s.request.headers.retain(|h| !h.key.eq_ignore_ascii_case(&key)))
        })?;
    }
    Ok(object)
}

/// Read, edit and write back the request URL's query rows.
fn edit_query<'js>(ctx: &Ctx<'js>, shared: &Shared, edit: impl FnOnce(&mut Vec<Param>)) -> Result<()> {
    ensure_writable(ctx, shared)?;
    with_state(ctx, shared, |s| {
        let mut url = Url::parse(&s.request.url);
        let mut params = url.query.as_ref().map(|q| params_of_query(&q.0)).unwrap_or_default();
        edit(&mut params);
        url.query = query_of_params(&params);
        s.request.url = url.build();
    })
}

fn url_object<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    {
        let shared = shared.clone();
        def(ctx, &object, "toString", move |ctx: Ctx<'js>| -> Result<String> { with_state(&ctx, &shared, |s| s.request.url.clone()) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "getHost", move |ctx: Ctx<'js>| -> Result<String> { with_state(&ctx, &shared, |s| Url::parse(&s.request.url).authority) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "getPath", move |ctx: Ctx<'js>| -> Result<String> { with_state(&ctx, &shared, |s| Url::parse(&s.request.url).path) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "getQueryString", move |ctx: Ctx<'js>| -> Result<String> {
            with_state(&ctx, &shared, |s| Url::parse(&s.request.url).query.map(|q| q.0).unwrap_or_default())
        })?;
    }
    let query = Object::new(ctx.clone())?;
    {
        let shared = shared.clone();
        def(ctx, &query, "get", move |ctx: Ctx<'js>, key: String| -> Result<Option<String>> {
            with_state(&ctx, &shared, |s| {
                let url = Url::parse(&s.request.url);
                url.query.map(|q| params_of_query(&q.0)).unwrap_or_default().into_iter().find(|p| p.key == key).map(|p| p.value)
            })
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &query, "has", move |ctx: Ctx<'js>, key: String| -> Result<bool> {
            with_state(&ctx, &shared, |s| {
                Url::parse(&s.request.url).query.map(|q| params_of_query(&q.0)).unwrap_or_default().iter().any(|p| p.key == key)
            })
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &query, "toObject", move |ctx: Ctx<'js>| -> Result<Object<'js>> {
            let pairs = with_state(&ctx, &shared, |s| {
                Url::parse(&s.request.url)
                    .query
                    .map(|q| params_of_query(&q.0))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| (p.key, p.value))
                    .collect::<Vec<_>>()
            })?;
            convert::pairs_to_object(&ctx, pairs)
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &query, "add", move |ctx: Ctx<'js>, item: Object<'js>| -> Result<()> {
            let (key, value) = key_value(&ctx, &item)?;
            edit_query(&ctx, &shared, |params| params.push(Param { key, value }))
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &query, "upsert", move |ctx: Ctx<'js>, item: Object<'js>| -> Result<()> {
            let (key, value) = key_value(&ctx, &item)?;
            edit_query(&ctx, &shared, |params| match params.iter_mut().find(|p| p.key == key) {
                Some(param) => param.value = value,
                None => params.push(Param { key, value }),
            })
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &query, "remove", move |ctx: Ctx<'js>, key: String| -> Result<()> {
            edit_query(&ctx, &shared, |params| params.retain(|p| p.key != key))
        })?;
    }
    object.set("query", query)?;
    Ok(object)
}

fn body_object<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    {
        let shared = shared.clone();
        object.prop(
            "mode",
            Accessor::from(move |ctx: Ctx<'js>| -> Result<&'static str> {
                with_state(&ctx, &shared, |s| match s.request.body {
                    ScriptBody::None => "none",
                    ScriptBody::Raw(_) => "raw",
                    ScriptBody::UrlEncoded(_) => "urlencoded",
                    ScriptBody::FormData(_) => "formdata",
                    ScriptBody::Other => "other",
                })
            })
            .enumerable(),
        )?;
    }
    {
        let get = shared.clone();
        let set = shared.clone();
        object.prop(
            "raw",
            Accessor::from(move |ctx: Ctx<'js>| -> Result<Option<String>> {
                with_state(&ctx, &get, |s| match &s.request.body {
                    ScriptBody::Raw(text) => Some(text.clone()),
                    _ => None,
                })
            })
            .set(move |ctx: Ctx<'js>, value: Value<'js>| -> Result<()> {
                ensure_writable(&ctx, &set)?;
                let text = var_text(&ctx, &value)?;
                with_state(&ctx, &set, |s| s.request.body = ScriptBody::Raw(text))
            })
            .enumerable(),
        )?;
    }
    for (name, urlencoded) in [("urlencoded", true), ("formdata", false)] {
        let shared = shared.clone();
        object.prop(
            name,
            Accessor::from(move |ctx: Ctx<'js>| -> Result<Array<'js>> {
                let rows = with_state(&ctx, &shared, |s| match (&s.request.body, urlencoded) {
                    (ScriptBody::UrlEncoded(rows), true) | (ScriptBody::FormData(rows), false) => rows.clone(),
                    _ => Vec::new(),
                })?;
                let mut items = Vec::new();
                for (key, value) in rows {
                    let item = Object::new(ctx.clone())?;
                    item.set("key", key)?;
                    item.set("value", value)?;
                    items.push(item.into_value());
                }
                convert::array_of(&ctx, items)
            })
            .enumerable(),
        )?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "update", move |ctx: Ctx<'js>, value: Value<'js>| -> Result<()> {
            ensure_writable(&ctx, &shared)?;
            let text = var_text(&ctx, &value)?;
            with_state(&ctx, &shared, |s| s.request.body = ScriptBody::Raw(text))
        })?;
    }
    Ok(object)
}

fn request_object<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    let (name, id) = with_state(ctx, shared, |s| (s.request.name.clone(), s.request.id.clone()))?;
    object.set("name", name)?;
    object.set("id", id)?;
    {
        let get = shared.clone();
        let set = shared.clone();
        object.prop(
            "method",
            Accessor::from(move |ctx: Ctx<'js>| -> Result<String> { with_state(&ctx, &get, |s| s.request.method.clone()) })
                .set(move |ctx: Ctx<'js>, value: String| -> Result<()> {
                    ensure_writable(&ctx, &set)?;
                    with_state(&ctx, &set, |s| s.request.method = value.to_ascii_uppercase())
                })
                .enumerable(),
        )?;
    }
    {
        let get = shared.clone();
        let set = shared.clone();
        object.prop(
            "url",
            Accessor::from(move |ctx: Ctx<'js>| -> Result<Object<'js>> { url_object(&ctx, &get) })
                .set(move |ctx: Ctx<'js>, value: Value<'js>| -> Result<()> {
                    ensure_writable(&ctx, &set)?;
                    let text = match value.as_object() {
                        Some(object) => {
                            let to_string: Function = object.get("toString")?;
                            to_string.call((This(object.clone()),))?
                        }
                        None => var_text(&ctx, &value)?,
                    };
                    with_state(&ctx, &set, |s| s.request.url = text)
                })
                .enumerable(),
        )?;
    }
    object.set("headers", headers_object(ctx, shared)?)?;
    object.set("body", body_object(ctx, shared)?)?;
    Ok(object)
}

// ----------------------------------------------------------------------------------------
// Responses

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
}

pub(crate) fn response_object<'js>(ctx: &Ctx<'js>, shared: &Shared, response: &ScriptResponse, assertions: bool) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set("code", response.code)?;
    object.set("status", response.status.clone())?;
    object.set("responseTime", response.response_time_ms as f64)?;
    object.set("responseSize", response.size as f64)?;
    let headers = Object::new(ctx.clone())?;
    {
        let list = response.headers.clone();
        def(ctx, &headers, "get", move |name: String| -> Option<String> { header_value(&list, &name) })?;
    }
    {
        let list = response.headers.clone();
        def(ctx, &headers, "has", move |name: String| -> bool { header_value(&list, &name).is_some() })?;
    }
    {
        let list = response.headers.clone();
        def(ctx, &headers, "toObject", move |ctx: Ctx<'js>| -> Result<Object<'js>> { convert::pairs_to_object(&ctx, list.clone()) })?;
    }
    object.set("headers", headers)?;
    let body = Arc::new(response.body.clone());
    {
        let body = body.clone();
        def(ctx, &object, "text", move || -> String { String::from_utf8_lossy(&body).into_owned() })?;
    }
    {
        let body = body.clone();
        def(ctx, &object, "json", move |ctx: Ctx<'js>| -> Result<Value<'js>> {
            let text = String::from_utf8_lossy(&body).into_owned();
            if let Err(err) = postguy_util::json::parse_json_str(&text, &postguy_util::json::Caps::default()) {
                return Err(throw(&ctx, &format!("JSONError: {err}")));
            }
            ctx.json_parse(text)
        })?;
    }
    let status = response.status.clone();
    def(ctx, &object, "reason", move || -> String { status.clone() })?;
    if assertions {
        let shared = shared.clone();
        object.prop("to", Accessor::from(move |ctx: Ctx<'js>| -> Result<Object<'js>> { expect::response_assertion(&ctx, &shared) }).enumerable())?;
    }
    Ok(object)
}

fn cookie_pairs(set_cookie: &[(String, String)]) -> Vec<(String, String)> {
    set_cookie
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .filter_map(|(_, v)| {
            let first = v.split(';').next()?;
            let (name, value) = first.split_once('=')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

fn cookies_object<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Object<'js>> {
    let pairs = move |ctx: &Ctx<'js>, shared: &Shared| -> Result<Vec<(String, String)>> {
        let (host, url, headers) =
            with_state(ctx, shared, |s| (s.host.clone(), s.request.url.clone(), s.response.as_ref().map(|r| r.headers.clone()).unwrap_or_default()))?;
        let mut out = cookie_pairs(&headers);
        for (name, value) in host.cookies_for(&url) {
            if !out.iter().any(|(k, _)| *k == name) {
                out.push((name, value));
            }
        }
        Ok(out)
    };
    let object = Object::new(ctx.clone())?;
    {
        let shared = shared.clone();
        def(ctx, &object, "get", move |ctx: Ctx<'js>, name: String| -> Result<Option<String>> {
            Ok(pairs(&ctx, &shared)?.into_iter().find(|(k, _)| *k == name).map(|(_, v)| v))
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "has", move |ctx: Ctx<'js>, name: String| -> Result<bool> { Ok(pairs(&ctx, &shared)?.iter().any(|(k, _)| *k == name)) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &object, "toObject", move |ctx: Ctx<'js>| -> Result<Object<'js>> { convert::pairs_to_object(&ctx, pairs(&ctx, &shared)?) })?;
    }
    def(ctx, &object, "jar", move |ctx: Ctx<'js>| -> Result<()> { Err(throw(&ctx, "pm.cookies.jar() is not supported yet")) })?;
    Ok(object)
}

// ----------------------------------------------------------------------------------------
// pm.test, pm.sendRequest, console and the small globals

fn record_test(state: &mut State, name: String, status: TestStatus, message: Option<String>) -> Option<usize> {
    if state.tests.len() >= state.limits.max_tests {
        if !state.tests_overflowed {
            state.tests_overflowed = true;
            let message = format!("more than {} tests; the rest were ignored", state.limits.max_tests);
            state.failures.push(ScriptFailure { phase: state.phase, origin: state.origin.clone(), message, line: None });
        }
        return None;
    }
    let phase = state.phase;
    state.tests.push(TestResult { name, status, message, phase });
    Some(state.tests.len() - 1)
}

fn test_object<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Function<'js>> {
    let run = {
        let shared = shared.clone();
        move |ctx: Ctx<'js>, name: String, body: Opt<Function<'js>>| -> Result<()> {
            let Some(body) = body.0 else {
                with_state(&ctx, &shared, |s| record_test(s, name, TestStatus::Passed, None))?;
                return Ok(());
            };
            match body.call::<_, Value>(()) {
                Ok(value) => {
                    if let Some(promise) = value.as_promise() {
                        let index =
                            with_state(&ctx, &shared, |s| record_test(s, name, TestStatus::Failed, Some("the test did not finish".to_string())))?;
                        let Some(index) = index else { return Ok(()) };
                        let on_ok = {
                            let shared = shared.clone();
                            Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> Result<()> {
                                with_state(&ctx, &shared, |s| {
                                    if let Some(test) = s.tests.get_mut(index) {
                                        test.status = TestStatus::Passed;
                                        test.message = None;
                                    }
                                })
                            })?
                        };
                        let on_err = {
                            let shared = shared.clone();
                            Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Opt<Value<'js>>| -> Result<()> {
                                let message = reason.0.as_ref().map(error_message).unwrap_or_default();
                                with_state(&ctx, &shared, |s| {
                                    if let Some(test) = s.tests.get_mut(index) {
                                        test.status = TestStatus::Failed;
                                        test.message = Some(message);
                                    }
                                })
                            })?
                        };
                        let then: Function = promise.get("then")?;
                        then.call::<_, Value>((This(promise.clone()), on_ok, on_err))?;
                    } else {
                        with_state(&ctx, &shared, |s| record_test(s, name, TestStatus::Passed, None))?;
                    }
                    Ok(())
                }
                Err(rquickjs::Error::Exception) => {
                    let caught = ctx.catch();
                    if caught.is_uncatchable_error() {
                        return Err(ctx.throw(caught));
                    }
                    let message = error_message(&caught);
                    with_state(&ctx, &shared, |s| record_test(s, name, TestStatus::Failed, Some(message)))?;
                    Ok(())
                }
                Err(other) => Err(other),
            }
        }
    };
    let test = Function::new(ctx.clone(), run)?.with_name("test")?;
    {
        let shared = shared.clone();
        let Some(test_object) = test.as_object() else {
            return Err(throw(ctx, "internal error: pm.test is not an object"));
        };
        def(ctx, test_object, "skip", move |ctx: Ctx<'js>, name: String, _body: Opt<Value<'js>>| -> Result<()> {
            with_state(&ctx, &shared, |s| record_test(s, name, TestStatus::Skipped, None))?;
            Ok(())
        })?;
    }
    Ok(test)
}

fn string_pairs<'js>(value: &Value<'js>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(array) = value.as_array() {
        for item in array.iter::<Object>().flatten() {
            let key: Value = item.get("key").unwrap_or_else(|_| Value::new_undefined(item.ctx().clone()));
            let val: Value = item.get("value").unwrap_or_else(|_| Value::new_undefined(item.ctx().clone()));
            if !key.is_undefined() {
                out.push((display_text(&key), display_text(&val)));
            }
        }
    } else if let Some(object) = value.as_object() {
        for entry in object.props::<String, Value>().flatten() {
            out.push((entry.0, display_text(&entry.1)));
        }
    }
    out
}

fn request_from_options<'js>(ctx: &Ctx<'js>, options: &Value<'js>) -> Result<ScriptRequest> {
    let mut request = ScriptRequest { name: "pm.sendRequest".to_string(), method: "GET".to_string(), ..ScriptRequest::default() };
    if let Some(url) = options.as_string() {
        request.url = url.to_string()?;
        return Ok(request);
    }
    let Some(object) = options.as_object() else {
        return Err(throw(ctx, "pm.sendRequest expects a URL or an options object"));
    };
    let url: Value = object.get("url")?;
    if let Some(url_object) = url.as_object() {
        let to_string: Function = url_object.get("toString")?;
        request.url = to_string.call((This(url_object.clone()),))?;
    } else {
        request.url = display_text(&url);
    }
    if let Ok(method) = object.get::<_, String>("method") {
        request.method = method.to_ascii_uppercase();
    }
    let header: Value = object.get("header")?;
    for (key, value) in string_pairs(&header) {
        request.headers.push(Header { key, value, enabled: true });
    }
    let body: Value = object.get("body")?;
    if let Some(body) = body.as_object() {
        let mode: String = body.get("mode").unwrap_or_default();
        match mode.as_str() {
            "raw" => request.body = ScriptBody::Raw(body.get("raw").unwrap_or_default()),
            "urlencoded" => {
                let rows: Value = body.get("urlencoded")?;
                request.body = ScriptBody::UrlEncoded(string_pairs(&rows));
            }
            "formdata" => {
                let rows: Value = body.get("formdata")?;
                request.body = ScriptBody::FormData(string_pairs(&rows));
            }
            _ => {}
        }
    }
    Ok(request)
}

fn send_request_fn<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Function<'js>> {
    let shared = shared.clone();
    Function::new(ctx.clone(), move |ctx: Ctx<'js>, options: Value<'js>, callback: Opt<Function<'js>>| -> Result<Value<'js>> {
        let request = request_from_options(&ctx, &options)?;
        let (host, over, max, cancelled) = with_state(&ctx, &shared, |s| {
            s.sends = s.sends.saturating_add(1);
            (s.host.clone(), s.sends > s.limits.max_send_requests, s.limits.max_send_requests, s.cancelled.clone())
        })?;
        if over {
            return Err(throw(&ctx, &format!("pm.sendRequest was called more than {max} times")));
        }
        if cancelled() {
            return Err(throw(&ctx, "the script was cancelled"));
        }
        let result = host.send_request(request);
        let shared_for_response = shared.clone();
        let (error, response): (Value<'js>, Value<'js>) = match &result {
            Ok(response) => (Value::new_null(ctx.clone()), response_object(&ctx, &shared_for_response, response, false)?.into_value()),
            Err(message) => (Exception::from_message(ctx.clone(), message)?.into_object().into_value(), Value::new_undefined(ctx.clone())),
        };
        match callback.0 {
            Some(callback) => {
                callback.call::<_, Value>((error, response))?;
                Ok(Value::new_undefined(ctx.clone()))
            }
            None => {
                let (promise, resolve, reject) = ctx.promise()?;
                if result.is_ok() {
                    resolve.call::<_, ()>((response,))?;
                } else {
                    reject.call::<_, ()>((error,))?;
                }
                Ok(promise.into_value())
            }
        }
    })
}

fn console_object<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Object<'js>> {
    let console = Object::new(ctx.clone())?;
    for (name, level) in
        [("log", LogLevel::Log), ("info", LogLevel::Info), ("warn", LogLevel::Warn), ("error", LogLevel::Error), ("debug", LogLevel::Debug)]
    {
        let shared = shared.clone();
        def(ctx, &console, name, move |ctx: Ctx<'js>, args: Rest<Value<'js>>| -> Result<()> {
            let mut message = args.0.iter().map(display_text).collect::<Vec<_>>().join(" ");
            if message.len() > LOG_LINE_CAP {
                let mut cut = LOG_LINE_CAP;
                while !message.is_char_boundary(cut) {
                    cut = cut.saturating_sub(1);
                }
                message.truncate(cut);
                message.push_str("… (truncated)");
            }
            with_state(&ctx, &shared, |s| {
                if s.logs.len() < s.limits.max_log_lines {
                    s.logs.push(LogLine { level, message });
                }
            })
        })?;
    }
    Ok(console)
}

fn next_request_fn<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Function<'js>> {
    let shared = shared.clone();
    Function::new(ctx.clone(), move |ctx: Ctx<'js>, target: Opt<Value<'js>>| -> Result<()> {
        let next = match target.0 {
            Some(value) if value.is_string() => NextRequest::Named(display_text(&value)),
            _ => NextRequest::Stop,
        };
        with_state(&ctx, &shared, |s| s.next = next)
    })
}

/// Install every global.
pub(crate) fn install<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<()> {
    let globals = ctx.globals();
    let pm = Object::new(ctx.clone())?;

    let info = with_state(ctx, shared, |s| s.info.clone())?;
    let info_object = Object::new(ctx.clone())?;
    info_object.set("eventName", info.event_name)?;
    info_object.set("iteration", info.iteration)?;
    info_object.set("iterationCount", info.iteration_count)?;
    info_object.set("requestName", info.request_name)?;
    info_object.set("requestId", info.request_id)?;
    pm.set("info", info_object)?;

    pm.set("globals", scope_object(ctx, shared, Scope::Globals)?)?;
    pm.set("collectionVariables", scope_object(ctx, shared, Scope::Collection)?)?;
    pm.set("environment", scope_object(ctx, shared, Scope::Environment)?)?;
    pm.set("variables", scope_object(ctx, shared, Scope::Variables)?)?;
    pm.set("iterationData", scope_object(ctx, shared, Scope::Data)?)?;
    pm.set("request", request_object(ctx, shared)?)?;
    if let Some(response) = with_state(ctx, shared, |s| s.response.clone())? {
        pm.set("response", response_object(ctx, shared, &response, true)?)?;
    }
    pm.set("cookies", cookies_object(ctx, shared)?)?;
    pm.set("test", test_object(ctx, shared)?)?;
    pm.set("sendRequest", send_request_fn(ctx, shared)?)?;
    {
        let shared = shared.clone();
        def(ctx, &pm, "expect", move |ctx: Ctx<'js>, value: Value<'js>| -> Result<Object<'js>> { expect::new_assertion(&ctx, &shared, value) })?;
    }

    let execution = Object::new(ctx.clone())?;
    execution.set("setNextRequest", next_request_fn(ctx, shared)?)?;
    {
        let shared = shared.clone();
        def(ctx, &execution, "skipRequest", move |ctx: Ctx<'js>| -> Result<()> {
            if with_state(&ctx, &shared, |s| s.phase == ScriptPhase::PostResponse)? {
                return Err(throw(&ctx, "pm.execution.skipRequest() can only be called in a pre-request script"));
            }
            with_state(&ctx, &shared, |s| s.skip = true)
        })?;
    }
    pm.set("execution", execution)?;
    globals.set("pm", pm)?;

    let postman = Object::new(ctx.clone())?;
    postman.set("setNextRequest", next_request_fn(ctx, shared)?)?;
    globals.set("postman", postman)?;
    globals.set("console", console_object(ctx, shared)?)?;

    def(ctx, &globals, "btoa", move |ctx: Ctx<'js>, text: String| -> Result<String> {
        let mut bytes = Vec::with_capacity(text.len());
        for c in text.chars() {
            match u8::try_from(u32::from(c)) {
                Ok(b) => bytes.push(b),
                Err(_) => return Err(throw(&ctx, "InvalidCharacterError: btoa only encodes Latin-1 text")),
            }
        }
        Ok(STANDARD.encode(bytes))
    })?;
    def(ctx, &globals, "atob", move |ctx: Ctx<'js>, text: String| -> Result<String> {
        let cleaned: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        match STANDARD.decode(cleaned.trim_end_matches('=')).or_else(|_| STANDARD.decode(&cleaned)) {
            Ok(bytes) => Ok(bytes.into_iter().map(char::from).collect()),
            Err(_) => Err(throw(&ctx, "InvalidCharacterError: the string to be decoded is not correctly encoded")),
        }
    })?;
    def(ctx, &globals, "require", move |ctx: Ctx<'js>, name: Opt<String>| -> Result<()> {
        Err(throw(&ctx, &format!("require('{}') is not available in Postguy scripts", name.0.unwrap_or_default())))
    })?;

    globals.prop(TIMERS, rquickjs::object::Property::from(Array::new(ctx.clone())?).writable())?;
    def(ctx, &globals, "setTimeout", move |ctx: Ctx<'js>, callback: Function<'js>, delay: Opt<f64>, args: Rest<Value<'js>>| -> Result<f64> {
        let timers: Array = ctx.globals().get(TIMERS)?;
        if timers.len() >= MAX_TIMERS {
            return Err(throw(&ctx, "too many pending timers"));
        }
        let id = (timers.len() + 1) as f64;
        let entry = Object::new(ctx.clone())?;
        entry.set("f", callback)?;
        entry.set("ms", delay.0.filter(|d| d.is_finite() && *d > 0.0).unwrap_or(0.0))?;
        entry.set("id", id)?;
        entry.set("args", convert::array_of(&ctx, args.0)?)?;
        timers.set(timers.len(), entry)?;
        Ok(id)
    })?;
    def(ctx, &globals, "clearTimeout", move |ctx: Ctx<'js>, id: Opt<f64>| -> Result<()> {
        let Some(id) = id.0 else { return Ok(()) };
        let timers: Array = ctx.globals().get(TIMERS)?;
        for entry in timers.iter::<Object>().flatten() {
            if entry.get::<_, f64>("id").is_ok_and(|candidate| candidate == id) {
                entry.set("cancelled", true)?;
            }
        }
        Ok(())
    })?;

    expect::install(ctx, shared)
}
