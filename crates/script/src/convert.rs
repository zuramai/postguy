//! JS value ⇄ plain Rust value conversion with hard caps, Chai-style printing and deep
//! equality. Conversion is the only place a hostile script's object graph is walked from Rust,
//! so depth and node count are capped (cycles hit the depth cap).

use rquickjs::{Array, Ctx, Object, Type, Value};
use serde_json::Value as Json;

/// Deepest nesting converted or compared.
pub(crate) const MAX_DEPTH: usize = 64;
/// Most nodes converted from one value.
pub(crate) const MAX_NODES: usize = 100_000;
/// Longest printed value.
const PRINT_CAP: usize = 1_000;

/// A JS value copied out of the engine.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Js {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Array(Vec<Js>),
    Object(Vec<(String, Js)>),
    Function,
    /// Symbols, BigInts and other values with no plain representation.
    Other(String),
}

/// The value is nested deeper or holds more nodes than the caps allow.
#[derive(Debug)]
pub(crate) struct TooBig;

pub(crate) const TOO_BIG_MESSAGE: &str = "value is too deeply nested or too large";

/// Copy `value` out of the engine.
pub(crate) fn from_value(value: &Value<'_>) -> Result<Js, TooBig> {
    let mut budget = MAX_NODES;
    walk(value, 0, &mut budget)
}

fn walk(value: &Value<'_>, depth: usize, budget: &mut usize) -> Result<Js, TooBig> {
    if depth > MAX_DEPTH || *budget == 0 {
        return Err(TooBig);
    }
    *budget = budget.saturating_sub(1);
    Ok(match value.type_of() {
        Type::Uninitialized | Type::Undefined => Js::Undefined,
        Type::Null => Js::Null,
        Type::Bool => Js::Bool(value.as_bool().unwrap_or(false)),
        Type::Int | Type::Float => Js::Num(value.as_number().unwrap_or(f64::NAN)),
        Type::String => Js::Str(value.as_string().and_then(|s| s.to_string().ok()).unwrap_or_default()),
        Type::Symbol => Js::Other("Symbol()".to_string()),
        Type::BigInt => Js::Other("BigInt".to_string()),
        Type::Function | Type::Constructor => Js::Function,
        Type::Array => {
            let mut items = Vec::new();
            if let Some(array) = value.as_array() {
                for item in array.iter::<Value>() {
                    match item {
                        Ok(item) => items.push(walk(&item, depth + 1, budget)?),
                        Err(_) => items.push(Js::Undefined),
                    }
                }
            }
            Js::Array(items)
        }
        Type::Object | Type::Exception | Type::Promise | Type::Proxy | Type::Module | Type::Unknown => {
            let mut entries = Vec::new();
            if let Some(object) = value.as_object() {
                for entry in object.props::<String, Value>() {
                    let Ok((key, item)) = entry else { continue };
                    entries.push((key, walk(&item, depth + 1, budget)?));
                }
            }
            Js::Object(entries)
        }
    })
}

impl Js {
    /// The JSON form. `undefined` and functions have none (`None`); inside containers they are
    /// dropped like `JSON.stringify` does, and in arrays they become `null`.
    pub(crate) fn to_json(&self) -> Option<Json> {
        match self {
            Js::Undefined | Js::Function | Js::Other(_) => None,
            Js::Null => Some(Json::Null),
            Js::Bool(b) => Some(Json::Bool(*b)),
            Js::Num(n) => Some(number_json(*n)),
            Js::Str(s) => Some(Json::String(s.clone())),
            Js::Array(items) => Some(Json::Array(items.iter().map(|i| i.to_json().unwrap_or(Json::Null)).collect())),
            Js::Object(entries) => {
                let mut map = serde_json::Map::new();
                for (key, item) in entries {
                    if let Some(json) = item.to_json() {
                        map.insert(key.clone(), json);
                    }
                }
                Some(Json::Object(map))
            }
        }
    }

    /// The JS `typeof`-ish type name Chai's `a()` uses.
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Js::Undefined => "undefined",
            Js::Null => "null",
            Js::Bool(_) => "boolean",
            Js::Num(_) => "number",
            Js::Str(_) => "string",
            Js::Array(_) => "array",
            Js::Object(_) => "object",
            Js::Function => "function",
            Js::Other(_) => "symbol",
        }
    }
}

fn number_json(n: f64) -> Json {
    if n.is_finite() {
        if n.fract() == 0.0 && n.abs() < 9.0e15 {
            return Json::from(n as i64);
        }
        if let Some(number) = serde_json::Number::from_f64(n) {
            return Json::Number(number);
        }
    }
    Json::Null
}

/// Chai's deep equality: structural, key order ignored, `NaN` equals `NaN`.
pub(crate) fn deep_eq(a: &Js, b: &Js) -> bool {
    deep_eq_at(a, b, 0)
}

fn deep_eq_at(a: &Js, b: &Js, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    match (a, b) {
        (Js::Num(x), Js::Num(y)) => x == y || (x.is_nan() && y.is_nan()),
        (Js::Array(x), Js::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| deep_eq_at(p, q, depth + 1)),
        (Js::Object(x), Js::Object(y)) => {
            x.len() == y.len()
                && x.iter().all(|(key, value)| y.iter().find(|(k, _)| k == key).is_some_and(|(_, other)| deep_eq_at(value, other, depth + 1)))
        }
        _ => a == b,
    }
}

/// Strict equality for values without identity (everything but arrays and objects).
pub(crate) fn strict_eq(a: &Js, b: &Js) -> bool {
    match (a, b) {
        (Js::Array(_), _) | (Js::Object(_), _) | (_, Js::Array(_)) | (_, Js::Object(_)) => false,
        (Js::Num(x), Js::Num(y)) => x == y,
        _ => a == b,
    }
}

/// Print a value the way Chai does: strings in single quotes, `[ 1, 2 ]`, `{ a: 1 }`.
pub(crate) fn inspect(value: &Js) -> String {
    let mut out = String::new();
    inspect_into(value, 0, &mut out);
    if out.chars().count() > PRINT_CAP {
        let cut: String = out.chars().take(PRINT_CAP).collect();
        return format!("{cut}…");
    }
    out
}

fn inspect_into(value: &Js, depth: usize, out: &mut String) {
    if out.len() > PRINT_CAP * 4 {
        return;
    }
    match value {
        Js::Undefined => out.push_str("undefined"),
        Js::Null => out.push_str("null"),
        Js::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Js::Num(n) => out.push_str(&number_text(*n)),
        Js::Str(s) => {
            out.push('\'');
            out.push_str(s);
            out.push('\'');
        }
        Js::Function => out.push_str("[Function]"),
        Js::Other(name) => out.push_str(name),
        Js::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
            } else if depth >= 3 {
                out.push_str("[Array]");
            } else {
                out.push_str("[ ");
                for (i, item) in items.iter().take(100).enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    inspect_into(item, depth + 1, out);
                }
                if items.len() > 100 {
                    out.push_str(", …");
                }
                out.push_str(" ]");
            }
        }
        Js::Object(entries) => {
            if entries.is_empty() {
                out.push_str("{}");
            } else if depth >= 3 {
                out.push_str("[Object]");
            } else {
                out.push_str("{ ");
                for (i, (key, item)) in entries.iter().take(100).enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(key);
                    out.push_str(": ");
                    inspect_into(item, depth + 1, out);
                }
                if entries.len() > 100 {
                    out.push_str(", …");
                }
                out.push_str(" }");
            }
        }
    }
}

/// JS number-to-string for the cases that matter here.
pub(crate) fn number_text(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string()
    } else if n.fract() == 0.0 && n.abs() < 1.0e21 {
        format!("{}", n as i128)
    } else {
        format!("{n}")
    }
}

/// A Rust JSON value as a JS value.
pub(crate) fn json_to_js<'js>(ctx: &Ctx<'js>, json: &Json) -> rquickjs::Result<Value<'js>> {
    ctx.json_parse(json.to_string())
}

/// A JS object from string pairs (later keys win).
pub(crate) fn pairs_to_object<'js>(ctx: &Ctx<'js>, pairs: impl IntoIterator<Item = (String, String)>) -> rquickjs::Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    for (key, value) in pairs {
        object.set(key, value)?;
    }
    Ok(object)
}

/// A JS array of values.
pub(crate) fn array_of<'js>(ctx: &Ctx<'js>, items: Vec<Value<'js>>) -> rquickjs::Result<Array<'js>> {
    let array = Array::new(ctx.clone())?;
    for (i, item) in items.into_iter().enumerate() {
        array.set(i, item)?;
    }
    Ok(array)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_matches_chai_style() {
        let value = Js::Object(vec![("a".into(), Js::Num(1.0)), ("b".into(), Js::Array(vec![Js::Str("x".into()), Js::Null]))]);
        assert_eq!(inspect(&value), "{ a: 1, b: [ 'x', null ] }");
        assert_eq!(inspect(&Js::Undefined), "undefined");
    }

    #[test]
    fn deep_eq_ignores_key_order_and_equates_nan() {
        let a = Js::Object(vec![("a".into(), Js::Num(f64::NAN)), ("b".into(), Js::Num(2.0))]);
        let b = Js::Object(vec![("b".into(), Js::Num(2.0)), ("a".into(), Js::Num(f64::NAN))]);
        assert!(deep_eq(&a, &b));
    }
}
