//! A Chai-style assertion subset for `pm.expect(...)` and `pm.response.to...`.
//!
//! An assertion is a JS object whose prototype (built once per run, kept on the global
//! object) holds the chain words, flag getters, terminal getters and methods. The value under
//! test and the flags live in hidden, non-enumerable properties of the instance, so no Rust
//! closure holds a JS value.

use rquickjs::function::{Rest, This};
use rquickjs::object::{Accessor, Property};
use rquickjs::{Ctx, Exception, Function, Object, Result, Value};

use crate::convert::{self, Js};
use crate::pm::{Shared, throw, with_state};

const PROTO: &str = "__pgExpect";
const RESPONSE_PROTO: &str = "__pgResponseExpect";
const VALUE: &str = "__v";
const NEGATE: &str = "__n";
const DEEP: &str = "__d";

/// Words that return the assertion unchanged, so chains read like English.
const CHAINS: &[&str] =
    &["to", "be", "been", "is", "that", "which", "and", "has", "have", "with", "at", "of", "same", "but", "does", "still", "own", "all", "any"];

/// The outcome of one assertion before negation is applied.
struct Check<'js> {
    pass: bool,
    positive: String,
    negative: String,
    /// Chai's `property` makes the property's value the new subject.
    retarget: Option<Value<'js>>,
}

impl<'js> Check<'js> {
    /// `expected {subject} to {desc}` / `expected {subject} to not {desc}`.
    fn plain(pass: bool, subject: &str, desc: &str) -> Check<'js> {
        Check { pass, positive: format!("expected {subject} to {desc}"), negative: format!("expected {subject} to not {desc}"), retarget: None }
    }
}

struct Assertion<'js> {
    value: Value<'js>,
    js: Js,
    neg: bool,
    deep: bool,
}

impl<'js> Assertion<'js> {
    fn read(ctx: &Ctx<'js>, this: &Object<'js>) -> Result<Assertion<'js>> {
        let value: Value = this.get(VALUE)?;
        let js = convert::from_value(&value).map_err(|_| throw(ctx, convert::TOO_BIG_MESSAGE))?;
        Ok(Assertion { value, js, neg: this.get(NEGATE).unwrap_or(false), deep: this.get(DEEP).unwrap_or(false) })
    }

    fn subject(&self) -> String {
        convert::inspect(&self.js)
    }
}

fn fail<'js>(ctx: &Ctx<'js>, message: &str) -> rquickjs::Error {
    match Exception::from_message(ctx.clone(), message) {
        Ok(exception) => {
            let object = exception.into_object();
            let _ = object.set("name", "AssertionError");
            ctx.throw(object.into_value())
        }
        Err(err) => err,
    }
}

fn conclude<'js>(ctx: &Ctx<'js>, this: &Object<'js>, neg: bool, check: Check<'js>) -> Result<()> {
    if check.pass == neg {
        return Err(fail(ctx, if neg { &check.negative } else { &check.positive }));
    }
    if let Some(value) = check.retarget {
        this.set(VALUE, value)?;
    }
    Ok(())
}

fn arg<'a, 'js>(ctx: &Ctx<'js>, args: &'a [Value<'js>], index: usize) -> Result<&'a Value<'js>> {
    args.get(index).ok_or_else(|| throw(ctx, "the assertion needs an argument"))
}

fn js_arg<'js>(ctx: &Ctx<'js>, args: &[Value<'js>], index: usize) -> Result<Js> {
    convert::from_value(arg(ctx, args, index)?).map_err(|_| throw(ctx, convert::TOO_BIG_MESSAGE))
}

fn num_arg<'js>(ctx: &Ctx<'js>, args: &[Value<'js>], index: usize) -> Result<f64> {
    arg(ctx, args, index)?.as_number().ok_or_else(|| throw(ctx, "the assertion needs a number argument"))
}

fn truthy(js: &Js) -> bool {
    match js {
        Js::Undefined | Js::Null => false,
        Js::Bool(b) => *b,
        Js::Num(n) => *n != 0.0 && !n.is_nan(),
        Js::Str(s) => !s.is_empty(),
        _ => true,
    }
}

fn is_container(js: &Js) -> bool {
    matches!(js, Js::Array(_) | Js::Object(_))
}

type Method = for<'js> fn(&Ctx<'js>, &Assertion<'js>, &str, &[Value<'js>]) -> Result<Check<'js>>;
type Terminal = for<'js> fn(&Ctx<'js>, &Assertion<'js>) -> Result<Check<'js>>;

// ---- terminals ---------------------------------------------------------------------------

fn t_ok<'js>(_: &Ctx<'js>, a: &Assertion<'js>) -> Result<Check<'js>> {
    Ok(Check::plain(truthy(&a.js), &a.subject(), "be ok"))
}
fn t_true<'js>(_: &Ctx<'js>, a: &Assertion<'js>) -> Result<Check<'js>> {
    Ok(Check::plain(a.js == Js::Bool(true), &a.subject(), "be true"))
}
fn t_false<'js>(_: &Ctx<'js>, a: &Assertion<'js>) -> Result<Check<'js>> {
    Ok(Check::plain(a.js == Js::Bool(false), &a.subject(), "be false"))
}
fn t_null<'js>(_: &Ctx<'js>, a: &Assertion<'js>) -> Result<Check<'js>> {
    Ok(Check::plain(a.js == Js::Null, &a.subject(), "be null"))
}
fn t_undefined<'js>(_: &Ctx<'js>, a: &Assertion<'js>) -> Result<Check<'js>> {
    Ok(Check::plain(a.js == Js::Undefined, &a.subject(), "be undefined"))
}
fn t_nan<'js>(_: &Ctx<'js>, a: &Assertion<'js>) -> Result<Check<'js>> {
    Ok(Check::plain(matches!(a.js, Js::Num(n) if n.is_nan()), &a.subject(), "be NaN"))
}
fn t_exist<'js>(_: &Ctx<'js>, a: &Assertion<'js>) -> Result<Check<'js>> {
    Ok(Check::plain(!matches!(a.js, Js::Undefined | Js::Null), &a.subject(), "exist"))
}
fn t_empty<'js>(_: &Ctx<'js>, a: &Assertion<'js>) -> Result<Check<'js>> {
    let empty = match &a.js {
        Js::Str(s) => s.is_empty(),
        Js::Array(items) => items.is_empty(),
        Js::Object(entries) => entries.is_empty(),
        _ => false,
    };
    Ok(Check::plain(empty, &a.subject(), "be empty"))
}

const TERMINALS: &[(&str, Terminal)] = &[
    ("ok", t_ok),
    ("true", t_true),
    ("false", t_false),
    ("null", t_null),
    ("undefined", t_undefined),
    ("NaN", t_nan),
    ("exist", t_exist),
    ("empty", t_empty),
];

// ---- methods -----------------------------------------------------------------------------

fn m_equal<'js>(ctx: &Ctx<'js>, a: &Assertion<'js>, name: &str, args: &[Value<'js>]) -> Result<Check<'js>> {
    let expected = js_arg(ctx, args, 0)?;
    let deep = a.deep || matches!(name, "eql" | "eqls");
    let pass = if deep {
        convert::deep_eq(&a.js, &expected)
    } else if is_container(&a.js) || is_container(&expected) {
        is_container(&a.js) && is_container(&expected) && a.value == *arg(ctx, args, 0)?
    } else {
        convert::strict_eq(&a.js, &expected)
    };
    let desc = format!("{}equal {}", if deep { "deeply " } else { "" }, convert::inspect(&expected));
    Ok(Check::plain(pass, &a.subject(), &desc))
}

fn m_compare<'js>(ctx: &Ctx<'js>, a: &Assertion<'js>, name: &str, args: &[Value<'js>]) -> Result<Check<'js>> {
    let limit = num_arg(ctx, args, 0)?;
    let Js::Num(actual) = a.js else {
        return Err(fail(ctx, &format!("expected {} to be a number", a.subject())));
    };
    let (pass, word) = match name {
        "above" | "gt" | "greaterThan" => (actual > limit, "above"),
        "below" | "lt" | "lessThan" => (actual < limit, "below"),
        "least" | "gte" => (actual >= limit, "at least"),
        _ => (actual <= limit, "at most"),
    };
    Ok(Check::plain(pass, &a.subject(), &format!("be {word} {}", convert::number_text(limit))))
}

fn m_within<'js>(ctx: &Ctx<'js>, a: &Assertion<'js>, _: &str, args: &[Value<'js>]) -> Result<Check<'js>> {
    let (lo, hi) = (num_arg(ctx, args, 0)?, num_arg(ctx, args, 1)?);
    let Js::Num(actual) = a.js else {
        return Err(fail(ctx, &format!("expected {} to be a number", a.subject())));
    };
    Ok(Check::plain(actual >= lo && actual <= hi, &a.subject(), &format!("be within {}..{}", convert::number_text(lo), convert::number_text(hi))))
}

fn m_type<'js>(ctx: &Ctx<'js>, a: &Assertion<'js>, name: &str, args: &[Value<'js>]) -> Result<Check<'js>> {
    let wanted = match arg(ctx, args, 0)?.as_string() {
        Some(s) => s.to_string()?.to_ascii_lowercase(),
        None => return Err(throw(ctx, "the type must be a string")),
    };
    let article = if name == "an" { "an" } else { "a" };
    Ok(Check::plain(a.js.type_name() == wanted, &a.subject(), &format!("be {article} {wanted}")))
}

fn js_contains(deep: bool, haystack: &[Js], needle: &Js) -> bool {
    haystack.iter().any(|item| {
        if deep || is_container(item) || is_container(needle) { convert::deep_eq(item, needle) } else { convert::strict_eq(item, needle) }
    })
}

fn m_include<'js>(ctx: &Ctx<'js>, a: &Assertion<'js>, _: &str, args: &[Value<'js>]) -> Result<Check<'js>> {
    let needle = js_arg(ctx, args, 0)?;
    let pass = match (&a.js, &needle) {
        (Js::Str(hay), Js::Str(n)) => hay.contains(n.as_str()),
        (Js::Array(items), _) => js_contains(a.deep, items, &needle),
        (Js::Object(entries), Js::Object(subset)) => {
            subset.iter().all(|(key, want)| entries.iter().find(|(k, _)| k == key).is_some_and(|(_, have)| convert::deep_eq(have, want)))
        }
        _ => false,
    };
    Ok(Check::plain(pass, &a.subject(), &format!("include {}", convert::inspect(&needle))))
}

fn m_property<'js>(ctx: &Ctx<'js>, a: &Assertion<'js>, _: &str, args: &[Value<'js>]) -> Result<Check<'js>> {
    let name = match arg(ctx, args, 0)?.as_string() {
        Some(s) => s.to_string()?,
        None => return Err(throw(ctx, "the property name must be a string")),
    };
    let subject = a.subject();
    let found: Option<Value<'js>> = match a.value.as_object() {
        Some(object) if object.contains_key(name.as_str())? => Some(object.get(name.as_str())?),
        _ => None,
    };
    let Some(value) = found else {
        return Ok(Check::plain(false, &subject, &format!("have property '{name}'")));
    };
    if args.len() < 2 {
        return Ok(Check {
            pass: true,
            positive: String::new(),
            negative: format!("expected {subject} to not have property '{name}'"),
            retarget: Some(value),
        });
    }
    let have = convert::from_value(&value).map_err(|_| throw(ctx, convert::TOO_BIG_MESSAGE))?;
    let want = js_arg(ctx, args, 1)?;
    let pass = if a.deep || is_container(&have) || is_container(&want) { convert::deep_eq(&have, &want) } else { convert::strict_eq(&have, &want) };
    Ok(Check {
        pass,
        positive: format!("expected {subject} to have property '{name}' of {}, but got {}", convert::inspect(&want), convert::inspect(&have)),
        negative: format!("expected {subject} to not have property '{name}' of {}", convert::inspect(&want)),
        retarget: if pass { Some(value) } else { None },
    })
}

fn m_length<'js>(ctx: &Ctx<'js>, a: &Assertion<'js>, _: &str, args: &[Value<'js>]) -> Result<Check<'js>> {
    let want = num_arg(ctx, args, 0)?;
    let subject = a.subject();
    let have = match &a.js {
        Js::Str(s) => s.encode_utf16().count(),
        Js::Array(items) => items.len(),
        _ => return Err(fail(ctx, &format!("expected {subject} to have a length"))),
    };
    let desc = format!("have a length of {}", convert::number_text(want));
    Ok(Check {
        pass: have as f64 == want,
        positive: format!("expected {subject} to {desc} but got {have}"),
        negative: format!("expected {subject} to not {desc}"),
        retarget: None,
    })
}

fn list_arg<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> Result<Vec<Js>> {
    if args.len() == 1
        && args.first().is_some_and(Value::is_array)
        && let Js::Array(items) = js_arg(ctx, args, 0)?
    {
        return Ok(items);
    }
    args.iter().map(|v| convert::from_value(v).map_err(|_| throw(ctx, convert::TOO_BIG_MESSAGE))).collect()
}

fn m_keys<'js>(ctx: &Ctx<'js>, a: &Assertion<'js>, _: &str, args: &[Value<'js>]) -> Result<Check<'js>> {
    let wanted = list_arg(ctx, args)?;
    let Js::Object(entries) = &a.js else {
        return Ok(Check::plain(false, &a.subject(), &format!("have keys {}", convert::inspect(&Js::Array(wanted)))));
    };
    let wanted_keys: Vec<String> = wanted.iter().map(|k| if let Js::Str(s) = k { s.clone() } else { convert::inspect(k) }).collect();
    let pass = wanted_keys.len() == entries.len() && wanted_keys.iter().all(|k| entries.iter().any(|(key, _)| key == k));
    Ok(Check::plain(pass, &a.subject(), &format!("have keys {}", convert::inspect(&Js::Array(wanted_keys.into_iter().map(Js::Str).collect())))))
}

fn m_members<'js>(ctx: &Ctx<'js>, a: &Assertion<'js>, _: &str, args: &[Value<'js>]) -> Result<Check<'js>> {
    let Js::Array(want) = js_arg(ctx, args, 0)? else {
        return Err(throw(ctx, "members expects an array"));
    };
    let desc = format!("have the same members as {}", convert::inspect(&Js::Array(want.clone())));
    let Js::Array(have) = &a.js else {
        return Ok(Check::plain(false, &a.subject(), &desc));
    };
    let mut remaining: Vec<&Js> = have.iter().collect();
    let mut pass = have.len() == want.len();
    for item in &want {
        match remaining.iter().position(|candidate| convert::deep_eq(candidate, item)) {
            Some(at) => {
                remaining.swap_remove(at);
            }
            None => pass = false,
        }
    }
    Ok(Check::plain(pass, &a.subject(), &desc))
}

fn m_one_of<'js>(ctx: &Ctx<'js>, a: &Assertion<'js>, _: &str, args: &[Value<'js>]) -> Result<Check<'js>> {
    let Js::Array(list) = js_arg(ctx, args, 0)? else {
        return Err(throw(ctx, "oneOf expects an array"));
    };
    let pass = js_contains(a.deep, &list, &a.js);
    Ok(Check::plain(pass, &a.subject(), &format!("be one of {}", convert::inspect(&Js::Array(list)))))
}

fn m_match<'js>(ctx: &Ctx<'js>, a: &Assertion<'js>, _: &str, args: &[Value<'js>]) -> Result<Check<'js>> {
    let Some(pattern) = arg(ctx, args, 0)?.as_object() else {
        return Err(throw(ctx, "match expects a regular expression"));
    };
    let test: Function = pattern.get("test")?;
    let to_string: Function = pattern.get("toString")?;
    let text: String = to_string.call((This(pattern.clone()),))?;
    let pass = match &a.js {
        Js::Str(s) => test.call::<_, bool>((This(pattern.clone()), s.clone()))?,
        _ => false,
    };
    Ok(Check::plain(pass, &a.subject(), &format!("match {text}")))
}

fn m_string<'js>(ctx: &Ctx<'js>, a: &Assertion<'js>, _: &str, args: &[Value<'js>]) -> Result<Check<'js>> {
    let needle = js_arg(ctx, args, 0)?;
    let pass = matches!((&a.js, &needle), (Js::Str(hay), Js::Str(n)) if hay.contains(n.as_str()));
    Ok(Check::plain(pass, &a.subject(), &format!("contain {}", convert::inspect(&needle))))
}

const METHODS: &[(&[&str], Method)] = &[
    (&["equal", "equals", "eq", "eql", "eqls"], m_equal),
    (&["above", "gt", "greaterThan", "below", "lt", "lessThan", "least", "gte", "most", "lte"], m_compare),
    (&["within"], m_within),
    (&["a", "an"], m_type),
    (&["include", "includes", "contain", "contains"], m_include),
    (&["property"], m_property),
    (&["lengthOf", "length"], m_length),
    (&["keys", "key"], m_keys),
    (&["members"], m_members),
    (&["oneOf"], m_one_of),
    (&["match", "matches"], m_match),
    (&["string"], m_string),
];

// ---- response assertions -----------------------------------------------------------------

type ResponseMethod = for<'js> fn(&Ctx<'js>, &crate::ScriptResponse, &[Value<'js>]) -> Result<Check<'js>>;
type ResponseTerminal = fn(&crate::ScriptResponse) -> (bool, &'static str);

fn r_status<'js>(ctx: &Ctx<'js>, r: &crate::ScriptResponse, args: &[Value<'js>]) -> Result<Check<'js>> {
    let wanted = arg(ctx, args, 0)?;
    if let Some(code) = wanted.as_number() {
        let desc = format!("have status code {}", convert::number_text(code));
        return Ok(Check {
            pass: f64::from(r.code) == code,
            positive: format!("expected response to {desc} but got {}", r.code),
            negative: format!("expected response to not {desc}"),
            retarget: None,
        });
    }
    let text = crate::pm::error_message(wanted);
    let desc = format!("have status reason '{text}'");
    Ok(Check {
        pass: r.status.eq_ignore_ascii_case(&text),
        positive: format!("expected response to {desc} but got '{}'", r.status),
        negative: format!("expected response to not {desc}"),
        retarget: None,
    })
}

fn r_header<'js>(ctx: &Ctx<'js>, r: &crate::ScriptResponse, args: &[Value<'js>]) -> Result<Check<'js>> {
    let name = crate::pm::error_message(arg(ctx, args, 0)?);
    let found = r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(&name)).map(|(_, v)| v.clone());
    if args.len() < 2 {
        return Ok(Check::plain(found.is_some(), "response", &format!("have header {name}")));
    }
    let wanted = crate::pm::error_message(arg(ctx, args, 1)?);
    let desc = format!("have header {name} with value '{wanted}'");
    let pass = found.as_deref() == Some(wanted.as_str());
    Ok(Check {
        pass,
        positive: format!("expected response to {desc}{}", found.as_ref().map(|v| format!(" but got '{v}'")).unwrap_or_default()),
        negative: format!("expected response to not {desc}"),
        retarget: None,
    })
}

fn r_body<'js>(ctx: &Ctx<'js>, r: &crate::ScriptResponse, args: &[Value<'js>]) -> Result<Check<'js>> {
    let wanted = crate::pm::error_message(arg(ctx, args, 0)?);
    let text = String::from_utf8_lossy(&r.body);
    let pass = text == wanted;
    Ok(Check::plain(pass, "response", &format!("have body '{wanted}'")))
}

fn r_json_body<'js>(ctx: &Ctx<'js>, r: &crate::ScriptResponse, args: &[Value<'js>]) -> Result<Check<'js>> {
    let text = String::from_utf8_lossy(&r.body).into_owned();
    let parsed = postguy_util::json::parse_json_str(&text, &postguy_util::json::Caps::default()).ok();
    let Some(path) = args.first() else {
        return Ok(Check::plain(parsed.is_some(), "response", "have a JSON body"));
    };
    let path = crate::pm::error_message(path);
    let mut current = parsed.as_ref();
    for key in path.split('.').filter(|k| !k.is_empty()) {
        current = current.and_then(|v| match v {
            serde_json::Value::Object(map) => map.get(key),
            serde_json::Value::Array(items) => key.parse::<usize>().ok().and_then(|i| items.get(i)),
            _ => None,
        });
    }
    let Some(found) = current else {
        return Ok(Check::plain(false, "response", &format!("have a JSON body with '{path}'")));
    };
    if args.len() < 2 {
        return Ok(Check::plain(true, "response", &format!("have a JSON body with '{path}'")));
    }
    let want = js_arg(ctx, args, 1)?;
    let pass = want.to_json().is_some_and(|w| w == *found);
    Ok(Check::plain(pass, "response", &format!("have a JSON body with '{path}' equal to {}", convert::inspect(&want))))
}

fn rt_ok(r: &crate::ScriptResponse) -> (bool, &'static str) {
    ((200..300).contains(&r.code), "ok (2xx)")
}
fn rt_success(r: &crate::ScriptResponse) -> (bool, &'static str) {
    ((200..300).contains(&r.code), "a success (2xx)")
}
fn rt_error(r: &crate::ScriptResponse) -> (bool, &'static str) {
    (r.code >= 400, "an error (4xx or 5xx)")
}
fn rt_client_error(r: &crate::ScriptResponse) -> (bool, &'static str) {
    ((400..500).contains(&r.code), "a client error (4xx)")
}
fn rt_server_error(r: &crate::ScriptResponse) -> (bool, &'static str) {
    ((500..600).contains(&r.code), "a server error (5xx)")
}
fn rt_not_found(r: &crate::ScriptResponse) -> (bool, &'static str) {
    (r.code == 404, "not found (404)")
}
fn rt_json(r: &crate::ScriptResponse) -> (bool, &'static str) {
    let json = r.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.to_ascii_lowercase().contains("json"));
    (json, "JSON (content-type contains json)")
}

const RESPONSE_METHODS: &[(&str, ResponseMethod)] = &[("status", r_status), ("header", r_header), ("body", r_body), ("jsonBody", r_json_body)];
const RESPONSE_TERMINALS: &[(&str, ResponseTerminal)] = &[
    ("ok", rt_ok),
    ("success", rt_success),
    ("error", rt_error),
    ("clientError", rt_client_error),
    ("serverError", rt_server_error),
    ("notFound", rt_not_found),
    ("json", rt_json),
];

// ---- installation ------------------------------------------------------------------------

fn chain_words<'js>(proto: &Object<'js>) -> Result<()> {
    for word in CHAINS {
        proto.prop(*word, Accessor::from(|This(this): This<Object<'js>>| this))?;
    }
    proto.prop(
        "not",
        Accessor::from(|This(this): This<Object<'js>>| -> Result<Object<'js>> {
            this.set(NEGATE, true)?;
            Ok(this)
        }),
    )?;
    proto.prop(
        "deep",
        Accessor::from(|This(this): This<Object<'js>>| -> Result<Object<'js>> {
            this.set(DEEP, true)?;
            Ok(this)
        }),
    )?;
    Ok(())
}

pub(crate) fn install<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<()> {
    let globals = ctx.globals();

    let proto = Object::new(ctx.clone())?;
    chain_words(&proto)?;
    for (name, terminal) in TERMINALS {
        let terminal: Terminal = *terminal;
        proto.prop(
            *name,
            Accessor::from(move |ctx: Ctx<'js>, This(this): This<Object<'js>>| -> Result<Object<'js>> {
                let assertion = Assertion::read(&ctx, &this)?;
                let check = terminal(&ctx, &assertion)?;
                conclude(&ctx, &this, assertion.neg, check)?;
                Ok(this)
            }),
        )?;
    }
    for (names, method) in METHODS {
        for name in *names {
            let method: Method = *method;
            let name: &'static str = name;
            let function = Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, This(this): This<Object<'js>>, Rest(args): Rest<Value<'js>>| -> Result<Object<'js>> {
                    let assertion = Assertion::read(&ctx, &this)?;
                    let check = method(&ctx, &assertion, name, &args)?;
                    conclude(&ctx, &this, assertion.neg, check)?;
                    Ok(this)
                },
            )?
            .with_name(name)?;
            proto.set(name, function)?;
        }
    }
    globals.prop(PROTO, Property::from(proto))?;

    let response = Object::new(ctx.clone())?;
    chain_words(&response)?;
    for (name, terminal) in RESPONSE_TERMINALS {
        let terminal: ResponseTerminal = *terminal;
        let shared = shared.clone();
        response.prop(
            *name,
            Accessor::from(move |ctx: Ctx<'js>, This(this): This<Object<'js>>| -> Result<Object<'js>> {
                let neg: bool = this.get(NEGATE).unwrap_or(false);
                let Some(r) = with_state(&ctx, &shared, |s| s.response.clone())? else {
                    return Err(throw(&ctx, "there is no response in a pre-request script"));
                };
                let (pass, what) = terminal(&r);
                conclude(&ctx, &this, neg, Check::plain(pass, "response", &format!("be {what}")))?;
                Ok(this)
            }),
        )?;
    }
    for (name, method) in RESPONSE_METHODS {
        let method: ResponseMethod = *method;
        let shared = shared.clone();
        let function =
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, This(this): This<Object<'js>>, Rest(args): Rest<Value<'js>>| -> Result<Object<'js>> {
                let neg: bool = this.get(NEGATE).unwrap_or(false);
                let Some(r) = with_state(&ctx, &shared, |s| s.response.clone())? else {
                    return Err(throw(&ctx, "there is no response in a pre-request script"));
                };
                let check = method(&ctx, &r, &args)?;
                conclude(&ctx, &this, neg, check)?;
                Ok(this)
            })?
            .with_name(*name)?;
        response.set(*name, function)?;
    }
    globals.prop(RESPONSE_PROTO, Property::from(response))?;
    Ok(())
}

pub(crate) fn new_assertion<'js>(ctx: &Ctx<'js>, _shared: &Shared, value: Value<'js>) -> Result<Object<'js>> {
    let proto: Object = ctx.globals().get(PROTO)?;
    let object = Object::new(ctx.clone())?;
    object.set_prototype(Some(&proto))?;
    object.prop(VALUE, Property::from(value).writable())?;
    object.prop(NEGATE, Property::from(false).writable())?;
    object.prop(DEEP, Property::from(false).writable())?;
    Ok(object)
}

pub(crate) fn response_assertion<'js>(ctx: &Ctx<'js>, _shared: &Shared) -> Result<Object<'js>> {
    let proto: Object = ctx.globals().get(RESPONSE_PROTO)?;
    let object = Object::new(ctx.clone())?;
    object.set_prototype(Some(&proto))?;
    object.prop(NEGATE, Property::from(false).writable())?;
    Ok(object)
}
