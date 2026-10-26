//! Bruno-compatible globals (`bru`, `req`, `res`, `test`, `expect`) and the pieces shared with
//! `pg`: the Bruno-style variable methods and the request methods. Everything calls the same
//! helpers `pm` uses; nothing is copied.

use postguy_model::ScriptPhase;
use rquickjs::function::{Opt, This};
use rquickjs::object::Accessor;
use rquickjs::{Ctx, Function, Object, Result, Value};
use serde_json::Value as Json;

use crate::convert;
use crate::pm::{
    Scope, Shared, def, display_text, ensure_writable_as, get_var, header_get, header_remove, header_upsert, interpolate, set_var, throw, unset_var,
    var_text, with_state,
};
use crate::{ScriptBody, ScriptResponse};

/// Deepest `..key` recursion in `res('..key')`.
const QUERY_DEPTH: usize = 64;

/// A function that always throws `"{name} is not supported in Postguy scripts"`.
pub(crate) fn unsupported<'js>(ctx: &Ctx<'js>, object: &Object<'js>, prop: &str, name: &str) -> Result<()> {
    let message = format!("{name} is not supported in Postguy scripts");
    def(ctx, object, prop, move |ctx: Ctx<'js>, _args: rquickjs::function::Rest<Value<'js>>| -> Result<()> { Err(throw(&ctx, &message)) })
}

/// `(get, set, has, delete)` method names of one scope.
type Names<'a> = (&'a str, &'a str, Option<&'a str>, Option<&'a str>);

fn scoped<'js>(ctx: &Ctx<'js>, object: &Object<'js>, shared: &Shared, scope: Scope, names: Names<'_>) -> Result<()> {
    let (get, set, has, delete) = names;
    {
        let shared = shared.clone();
        def(ctx, object, get, move |ctx: Ctx<'js>, key: String| get_var(&ctx, &shared, scope, &key))?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, set, move |ctx: Ctx<'js>, key: String, value: Value<'js>, _options: Opt<Value<'js>>| -> Result<()> {
            set_var(&ctx, &shared, scope, key, &value)
        })?;
    }
    if let Some(has) = has {
        let shared = shared.clone();
        def(ctx, object, has, move |ctx: Ctx<'js>, key: String| -> Result<bool> { Ok(!get_var(&ctx, &shared, scope, &key)?.is_undefined()) })?;
    }
    if let Some(delete) = delete {
        let shared = shared.clone();
        def(ctx, object, delete, move |ctx: Ctx<'js>, key: String| -> Result<()> { unset_var(&ctx, &shared, scope, &key) })?;
    }
    Ok(())
}

/// The Bruno-style variable methods, installed on both `pg` and `bru`.
pub(crate) fn install_var_methods<'js>(ctx: &Ctx<'js>, object: &Object<'js>, shared: &Shared) -> Result<()> {
    scoped(ctx, object, shared, Scope::Runtime, ("getVar", "setVar", Some("hasVar"), Some("deleteVar")))?;
    scoped(ctx, object, shared, Scope::Environment, ("getEnvVar", "setEnvVar", Some("hasEnvVar"), Some("deleteEnvVar")))?;
    scoped(ctx, object, shared, Scope::Globals, ("getGlobalVar", "setGlobalVar", None, None))?;
    scoped(ctx, object, shared, Scope::Globals, ("getGlobalEnvVar", "setGlobalEnvVar", None, None))?;
    scoped(ctx, object, shared, Scope::Collection, ("getCollectionVar", "setCollectionVar", None, None))?;
    {
        let shared = shared.clone();
        def(ctx, object, "getFolderVar", move |ctx: Ctx<'js>, key: String| get_var(&ctx, &shared, Scope::Folders, &key))?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "getRequestVar", move |ctx: Ctx<'js>, key: String| -> Result<Option<String>> {
            with_state(&ctx, &shared, |s| s.vars.locals.get(&key).cloned())
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "getEnvName", move |ctx: Ctx<'js>| -> Result<Option<String>> {
            with_state(&ctx, &shared, |s| s.vars.environment.as_ref().map(|(name, _)| name.clone()))
        })?;
    }
    {
        // Scripts never see OS environment variables: imported `.env` values are environment
        // secrets, so this walks the normal scope chain.
        let shared = shared.clone();
        def(ctx, object, "getProcessEnv", move |ctx: Ctx<'js>, key: String| get_var(&ctx, &shared, Scope::Variables, &key))?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "interpolate", move |ctx: Ctx<'js>, text: Value<'js>| -> Result<String> {
            let text = display_text(&text);
            with_state(&ctx, &shared, |s| interpolate(&s.vars, &text))
        })?;
    }
    Ok(())
}

/// The request methods shared by `pg.request` and `req`. `label` names the object in the
/// read-only error.
pub(crate) fn install_request_methods<'js>(ctx: &Ctx<'js>, object: &Object<'js>, shared: &Shared, label: &'static str) -> Result<()> {
    {
        let shared = shared.clone();
        def(ctx, object, "getUrl", move |ctx: Ctx<'js>| -> Result<String> { with_state(&ctx, &shared, |s| s.request.url.clone()) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "setUrl", move |ctx: Ctx<'js>, url: Value<'js>| -> Result<()> { set_url(&ctx, &shared, label, &url) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "getMethod", move |ctx: Ctx<'js>| -> Result<String> { with_state(&ctx, &shared, |s| s.request.method.clone()) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "setMethod", move |ctx: Ctx<'js>, method: String| -> Result<()> { set_method(&ctx, &shared, label, method) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "getName", move |ctx: Ctx<'js>| -> Result<String> { with_state(&ctx, &shared, |s| s.request.name.clone()) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "getHeader", move |ctx: Ctx<'js>, name: String| -> Result<Option<String>> {
            with_state(&ctx, &shared, |s| header_get(s, &name))
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "getHeaders", move |ctx: Ctx<'js>| -> Result<Object<'js>> { headers_snapshot(&ctx, &shared) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "setHeader", move |ctx: Ctx<'js>, name: String, value: Value<'js>| -> Result<()> {
            ensure_writable_as(&ctx, &shared, label)?;
            let value = display_text(&value);
            with_state(&ctx, &shared, |s| header_upsert(s, name, value))
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "setHeaders", move |ctx: Ctx<'js>, headers: Object<'js>| -> Result<()> { set_headers(&ctx, &shared, label, &headers) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "deleteHeader", move |ctx: Ctx<'js>, name: String| -> Result<()> {
            ensure_writable_as(&ctx, &shared, label)?;
            with_state(&ctx, &shared, |s| header_remove(s, &name))
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "getBody", move |ctx: Ctx<'js>| -> Result<Value<'js>> { get_body(&ctx, &shared) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, object, "setBody", move |ctx: Ctx<'js>, body: Value<'js>| -> Result<()> { set_body(&ctx, &shared, label, &body) })?;
    }
    Ok(())
}

fn set_url<'js>(ctx: &Ctx<'js>, shared: &Shared, label: &str, url: &Value<'js>) -> Result<()> {
    ensure_writable_as(ctx, shared, label)?;
    let text = display_text(url);
    with_state(ctx, shared, |s| s.request.url = text)
}

fn set_method<'js>(ctx: &Ctx<'js>, shared: &Shared, label: &str, method: String) -> Result<()> {
    ensure_writable_as(ctx, shared, label)?;
    with_state(ctx, shared, |s| s.request.method = method.to_ascii_uppercase())
}

fn set_headers<'js>(ctx: &Ctx<'js>, shared: &Shared, label: &str, headers: &Object<'js>) -> Result<()> {
    ensure_writable_as(ctx, shared, label)?;
    for entry in headers.props::<String, Value>() {
        let (name, value) = entry?;
        let value = display_text(&value);
        with_state(ctx, shared, |s| header_upsert(s, name, value))?;
    }
    Ok(())
}

fn headers_snapshot<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Object<'js>> {
    let pairs =
        with_state(ctx, shared, |s| s.request.headers.iter().filter(|h| h.enabled).map(|h| (h.key.clone(), h.value.clone())).collect::<Vec<_>>())?;
    convert::pairs_to_object(ctx, pairs)
}

/// `getBody()`: raw text parsed as JSON when it parses, else the string.
fn get_body<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Value<'js>> {
    let body = with_state(ctx, shared, |s| s.request.body.clone())?;
    match body {
        ScriptBody::Raw(text) => match ctx.json_parse(text.clone()) {
            Ok(value) => Ok(value),
            Err(rquickjs::Error::Exception) => {
                // Not JSON: the exception is expected, the text is the body.
                let _ = ctx.catch();
                Ok(rquickjs::String::from_str(ctx.clone(), &text)?.into_value())
            }
            Err(other) => Err(other),
        },
        ScriptBody::UrlEncoded(rows) | ScriptBody::FormData(rows) => {
            let mut items = Vec::new();
            for (key, value) in rows {
                let item = Object::new(ctx.clone())?;
                item.set("key", key)?;
                item.set("value", value)?;
                items.push(item.into_value());
            }
            Ok(convert::array_of(ctx, items)?.into_value())
        }
        ScriptBody::None | ScriptBody::Other => Ok(Value::new_undefined(ctx.clone())),
    }
}

/// `setBody(v)`: a string as-is, anything else compact JSON.
fn set_body<'js>(ctx: &Ctx<'js>, shared: &Shared, label: &str, body: &Value<'js>) -> Result<()> {
    ensure_writable_as(ctx, shared, label)?;
    if body.is_undefined() {
        return Err(throw(ctx, &format!("{label}.setBody expects a string or a JSON value")));
    }
    let text = var_text(ctx, body)?;
    with_state(ctx, shared, |s| s.request.body = ScriptBody::Raw(text))
}

// ----------------------------------------------------------------------------------------
// req

fn req_object<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Object<'js>> {
    let req = Object::new(ctx.clone())?;
    install_request_methods(ctx, &req, shared, "req")?;
    {
        let get = shared.clone();
        let set = shared.clone();
        req.prop(
            "url",
            Accessor::from(move |ctx: Ctx<'js>| -> Result<String> { with_state(&ctx, &get, |s| s.request.url.clone()) })
                .set(move |ctx: Ctx<'js>, value: Value<'js>| -> Result<()> { set_url(&ctx, &set, "req", &value) })
                .enumerable(),
        )?;
    }
    {
        let get = shared.clone();
        let set = shared.clone();
        req.prop(
            "method",
            Accessor::from(move |ctx: Ctx<'js>| -> Result<String> { with_state(&ctx, &get, |s| s.request.method.clone()) })
                .set(move |ctx: Ctx<'js>, value: String| -> Result<()> { set_method(&ctx, &set, "req", value) })
                .enumerable(),
        )?;
    }
    {
        let get = shared.clone();
        let set = shared.clone();
        req.prop(
            "headers",
            Accessor::from(move |ctx: Ctx<'js>| -> Result<Object<'js>> { headers_snapshot(&ctx, &get) })
                .set(move |ctx: Ctx<'js>, value: Object<'js>| -> Result<()> { set_headers(&ctx, &set, "req", &value) })
                .enumerable(),
        )?;
    }
    {
        let get = shared.clone();
        let set = shared.clone();
        req.prop(
            "body",
            Accessor::from(move |ctx: Ctx<'js>| -> Result<Value<'js>> { get_body(&ctx, &get) })
                .set(move |ctx: Ctx<'js>, value: Value<'js>| -> Result<()> { set_body(&ctx, &set, "req", &value) })
                .enumerable(),
        )?;
    }
    Ok(req)
}

// ----------------------------------------------------------------------------------------
// res

enum Tok {
    Key(String),
    Index(usize),
    Deep(String),
}

fn tokenize(path: &str) -> Vec<Tok> {
    let chars: Vec<char> = path.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let read_key = |from: usize| -> (String, usize) {
        let mut j = from;
        let mut key = String::new();
        while let Some(c) = chars.get(j) {
            if *c == '.' || *c == '[' {
                break;
            }
            key.push(*c);
            j += 1;
        }
        (key, j)
    };
    while let Some(c) = chars.get(i) {
        if *c == '.' && chars.get(i + 1) == Some(&'.') {
            let (key, next) = read_key(i + 2);
            out.push(Tok::Deep(key));
            i = next;
        } else if *c == '.' {
            i += 1;
        } else if *c == '[' {
            let mut j = i + 1;
            let mut inner = String::new();
            while let Some(d) = chars.get(j) {
                j += 1;
                if *d == ']' {
                    break;
                }
                inner.push(*d);
            }
            match inner.trim().parse::<usize>() {
                Ok(n) => out.push(Tok::Index(n)),
                Err(_) => out.push(Tok::Key(inner.trim().trim_matches(['"', '\'']).to_string())),
            }
            i = j;
        } else {
            let (key, next) = read_key(i);
            out.push(Tok::Key(key));
            i = next;
        }
    }
    out
}

fn collect_deep(json: &Json, key: &str, depth: usize, out: &mut Vec<Json>) {
    if depth > QUERY_DEPTH {
        return;
    }
    match json {
        Json::Object(map) => {
            for (k, v) in map {
                if k == key {
                    out.push(v.clone());
                }
                collect_deep(v, key, depth + 1, out);
            }
        }
        Json::Array(items) => {
            for item in items {
                collect_deep(item, key, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// `res('a.b[0].c')` over a JSON body; `..key` gathers every `key` at any depth.
fn query(json: &Json, path: &str) -> Option<Json> {
    let mut current = vec![json.clone()];
    let mut multi = false;
    for token in tokenize(path) {
        current = match token {
            Tok::Key(key) => current.iter().filter_map(|v| v.get(&key).cloned()).collect(),
            Tok::Index(n) => current.iter().filter_map(|v| v.get(n).cloned()).collect(),
            Tok::Deep(key) => {
                multi = true;
                let mut out = Vec::new();
                for v in &current {
                    collect_deep(v, &key, 0, &mut out);
                }
                out
            }
        };
    }
    if multi { Some(Json::Array(current)) } else { current.into_iter().next() }
}

fn res_function<'js>(ctx: &Ctx<'js>, response: &ScriptResponse) -> Result<Function<'js>> {
    let text = String::from_utf8_lossy(&response.body).into_owned();
    let json: Option<Json> = postguy_util::json::parse_json_str(&text, &postguy_util::json::Caps::default()).ok();
    let query_json = json.clone();
    let res = Function::new(ctx.clone(), move |ctx: Ctx<'js>, path: String| -> Result<Value<'js>> {
        match query_json.as_ref().and_then(|j| query(j, &path)) {
            Some(found) => convert::json_to_js(&ctx, &found),
            None => Ok(Value::new_undefined(ctx.clone())),
        }
    })?;
    let Some(object) = res.as_object() else {
        return Err(throw(ctx, "internal error: res is not an object"));
    };
    object.set("status", response.code)?;
    object.set("statusText", response.status.clone())?;
    object.set("responseTime", response.response_time_ms as f64)?;
    let mut headers: Vec<(String, String)> = Vec::new();
    for (key, value) in &response.headers {
        let key = key.to_ascii_lowercase();
        match headers.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => {
                slot.1.push_str(", ");
                slot.1.push_str(value);
            }
            None => headers.push((key, value.clone())),
        }
    }
    let header_bytes: u64 = response.headers.iter().map(|(k, v)| (k.len() + v.len() + 4) as u64).sum();
    let body_bytes = response.size;
    let body = match &json {
        Some(json) => convert::json_to_js(ctx, json).ok(),
        None => None,
    };
    let body = match body {
        Some(body) => body,
        None => rquickjs::String::from_str(ctx.clone(), &text)?.into_value(),
    };
    object.set("body", body)?;
    {
        let list = headers.clone();
        object.set("headers", convert::pairs_to_object(ctx, list)?)?;
    }
    {
        let code = response.code;
        def(ctx, object, "getStatus", move || -> u16 { code })?;
    }
    {
        let status = response.status.clone();
        def(ctx, object, "getStatusText", move || -> String { status.clone() })?;
    }
    {
        let list = headers.clone();
        def(ctx, object, "getHeader", move |name: String| -> Option<String> {
            let name = name.to_ascii_lowercase();
            list.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone())
        })?;
    }
    def(ctx, object, "getHeaders", move |this: This<Object<'js>>| -> Result<Value<'js>> { this.0.get("headers") })?;
    def(ctx, object, "getBody", move |this: This<Object<'js>>| -> Result<Value<'js>> { this.0.get("body") })?;
    {
        let time = response.response_time_ms as f64;
        def(ctx, object, "getResponseTime", move || -> f64 { time })?;
    }
    def(ctx, object, "getSize", move |ctx: Ctx<'js>| -> Result<Object<'js>> {
        let size = Object::new(ctx.clone())?;
        size.set("body", body_bytes as f64)?;
        size.set("headers", header_bytes as f64)?;
        size.set("total", body_bytes.saturating_add(header_bytes) as f64)?;
        Ok(size)
    })?;
    Ok(res)
}

// ----------------------------------------------------------------------------------------

/// Install `bru`, `req`, `res` (post-response only), `test` and `expect`.
pub(crate) fn install<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<()> {
    let globals = ctx.globals();
    let pm: Object = globals.get("pm")?;
    let pg: Object = globals.get("pg")?;
    let execution: Object = pm.get("execution")?;

    let bru = Object::new(ctx.clone())?;
    install_var_methods(ctx, &bru, shared)?;
    bru.set("sendRequest", pm.get::<_, Value>("sendRequest")?)?;
    bru.set("setNextRequest", execution.get::<_, Value>("setNextRequest")?)?;
    let runner = Object::new(ctx.clone())?;
    runner.set("setNextRequest", execution.get::<_, Value>("setNextRequest")?)?;
    runner.set("skipRequest", execution.get::<_, Value>("skipRequest")?)?;
    runner.set("stopExecution", pg.get::<_, Value>("stopExecution")?)?;
    bru.set("runner", runner)?;
    unsupported(ctx, &bru, "sleep", "bru.sleep")?;
    unsupported(ctx, &bru, "cwd", "bru.cwd")?;
    unsupported(ctx, &bru, "runRequest", "bru.runRequest")?;
    globals.set("bru", bru)?;

    globals.set("req", req_object(ctx, shared)?)?;
    let phase = with_state(ctx, shared, |s| s.phase)?;
    if phase == ScriptPhase::PostResponse
        && let Some(response) = with_state(ctx, shared, |s| s.response.clone())?
    {
        globals.set("res", res_function(ctx, &response)?)?;
    }
    globals.set("test", pm.get::<_, Value>("test")?)?;
    globals.set("expect", pm.get::<_, Value>("expect")?)?;
    Ok(())
}
