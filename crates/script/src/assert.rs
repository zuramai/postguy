//! The Tests tab's assertion table: each enabled row `expr operator value` is evaluated after
//! the post-response scripts. The expression is JS (so `res.status`, `res.body.id`,
//! `res.headers['content-type']` all work); the operator is plain Rust, except `matches`,
//! which asks the JS `RegExp` constructor so the dialect is the one scripts use.

use postguy_model::{AssertOperator as Op, Assertion, TestKind, TestStatus};
use rquickjs::context::EvalOptions;
use rquickjs::function::This;
use rquickjs::{Constructor, Ctx, Function, Object, Result, Type, Value};
use serde_json::Value as Json;

use crate::convert;
use crate::pm::{Shared, error_message, interpolate, record_result, with_state};

/// Longest actual value quoted in a failure message.
const ACTUAL_CAP: usize = 200;

/// What an expression produced.
#[derive(Debug, Clone, PartialEq)]
enum Actual {
    Undefined,
    Json(Json),
}

enum Evaluated {
    Value(Actual),
    Threw(String),
}

/// Run every enabled assertion in order, recording one result each. `Err` is only an
/// uncatchable engine error (timeout, cancel, memory), left pending for the caller.
pub(crate) fn run_assertions<'js>(ctx: &Ctx<'js>, shared: &Shared, assertions: &[Assertion]) -> Result<()> {
    for assertion in assertions.iter().filter(|a| a.enabled) {
        let op = assertion.operator;
        let expr = assertion.expr.trim();
        let stored = assertion.value.trim();
        let name = if op.is_unary() { format!("{expr} {}", op.token()) } else { format!("{expr} {} {stored}", op.token()) };
        let raw = with_state(ctx, shared, |s| interpolate(&s.vars, &assertion.value))?.trim().to_string();
        let (status, message) = match evaluate_expr(ctx, expr)? {
            Evaluated::Threw(message) => (TestStatus::Failed, Some(format!("could not evaluate {expr}: {message}"))),
            Evaluated::Value(actual) => match check(ctx, op, &actual, &raw)? {
                Verdict::Pass => (TestStatus::Passed, None),
                Verdict::Fail(note) => {
                    let wanted = if op.is_unary() { format!("{expr} {}", op.label()) } else { format!("{expr} {} {raw}", op.label()) };
                    let mut message = format!("expected {wanted}, got {}", actual_text(&actual));
                    if let Some(note) = note {
                        message.push_str("; ");
                        message.push_str(&note);
                    }
                    (TestStatus::Failed, Some(message))
                }
            },
        };
        with_state(ctx, shared, |s| record_result(s, name, status, message, TestKind::Assertion))?;
    }
    Ok(())
}

fn evaluate_expr<'js>(ctx: &Ctx<'js>, expr: &str) -> Result<Evaluated> {
    let source = format!("(function(){{return ({expr});\n}})()");
    let mut options = EvalOptions::default();
    options.global = true;
    options.strict = false;
    options.filename = Some("assertion".to_string());
    match ctx.eval_with_options::<Value, _>(source, options) {
        Ok(value) => to_actual(ctx, &value),
        Err(rquickjs::Error::Exception) => caught(ctx),
        Err(other) => Ok(Evaluated::Threw(other.to_string())),
    }
}

fn caught(ctx: &Ctx<'_>) -> Result<Evaluated> {
    caught_message(ctx).map(Evaluated::Threw)
}

fn to_actual<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Evaluated> {
    let actual = match value.type_of() {
        Type::Uninitialized | Type::Undefined | Type::Function | Type::Constructor | Type::Symbol | Type::BigInt => Actual::Undefined,
        Type::Null => Actual::Json(Json::Null),
        Type::Bool => Actual::Json(Json::Bool(value.as_bool().unwrap_or(false))),
        Type::Int | Type::Float => {
            let n = value.as_number().unwrap_or(f64::NAN);
            if n.is_finite() { Actual::Json(convert::number_json(n)) } else { Actual::Undefined }
        }
        Type::String => Actual::Json(Json::String(value.as_string().and_then(|s| s.to_string().ok()).unwrap_or_default())),
        _ => match ctx.json_stringify(value.clone()) {
            Ok(Some(text)) => match text.to_string().ok().and_then(|t| serde_json::from_str::<Json>(&t).ok()) {
                Some(json) => Actual::Json(json),
                None => Actual::Undefined,
            },
            Ok(None) => Actual::Undefined,
            Err(rquickjs::Error::Exception) => return caught(ctx),
            Err(other) => return Ok(Evaluated::Threw(other.to_string())),
        },
    };
    Ok(Evaluated::Value(actual))
}

// ----------------------------------------------------------------------------------------
// Value parsing and comparison

/// Literals, numbers, quoted strings and JSON containers; anything else is the raw string.
fn parse_value(raw: &str) -> Actual {
    match raw {
        "undefined" => return Actual::Undefined,
        "true" => return Actual::Json(Json::Bool(true)),
        "false" => return Actual::Json(Json::Bool(false)),
        "null" => return Actual::Json(Json::Null),
        _ => {}
    }
    if let Ok(number @ Json::Number(_)) = serde_json::from_str::<Json>(raw) {
        return Actual::Json(number);
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes.first().copied().unwrap_or(0);
        let last = bytes.last().copied().unwrap_or(0);
        if first == last && (first == b'"' || first == b'\'') {
            if first == b'"'
                && let Ok(Json::String(s)) = serde_json::from_str::<Json>(raw)
            {
                return Actual::Json(Json::String(s));
            }
            return Actual::Json(Json::String(raw.get(1..raw.len() - 1).unwrap_or("").to_string()));
        }
    }
    if (raw.starts_with('[') || raw.starts_with('{'))
        && let Ok(json) = serde_json::from_str::<Json>(raw)
    {
        return Actual::Json(json);
    }
    Actual::Json(Json::String(raw.to_string()))
}

fn json_eq(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => x.as_f64() == y.as_f64(),
        (Json::Array(x), Json::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_eq(p, q)),
        (Json::Object(x), Json::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w))),
        _ => a == b,
    }
}

fn deep_equal(a: &Actual, b: &Actual) -> bool {
    match (a, b) {
        (Actual::Undefined, Actual::Undefined) => true,
        (Actual::Json(x), Actual::Json(y)) => json_eq(x, y),
        _ => false,
    }
}

/// `eq` semantics: deep equal to the parsed value, or a string equal to the raw text.
fn eq_value(actual: &Actual, parsed: &Actual, raw: &str) -> bool {
    deep_equal(actual, parsed) || matches!(actual, Actual::Json(Json::String(s)) if s == raw)
}

fn num_of(actual: &Actual) -> Option<f64> {
    match actual {
        Actual::Json(Json::Number(n)) => n.as_f64(),
        Actual::Json(Json::String(s)) => s.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
        _ => None,
    }
}

fn is_empty(actual: &Actual) -> bool {
    match actual {
        Actual::Json(Json::String(s)) => s.is_empty(),
        Actual::Json(Json::Array(a)) => a.is_empty(),
        Actual::Json(Json::Object(o)) => o.is_empty(),
        _ => false,
    }
}

fn truthy(actual: &Actual) -> bool {
    match actual {
        Actual::Undefined | Actual::Json(Json::Null) => false,
        Actual::Json(Json::Bool(b)) => *b,
        Actual::Json(Json::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Actual::Json(Json::String(s)) => !s.is_empty(),
        Actual::Json(_) => true,
    }
}

/// The candidates of `in`/`notIn`: a JSON array, or comma-separated items.
fn candidates(parsed: &Actual, raw: &str) -> Vec<(Actual, String)> {
    if let Actual::Json(Json::Array(items)) = parsed {
        return items.iter().map(|item| (Actual::Json(item.clone()), json_text(item))).collect();
    }
    raw.split(',').map(|item| item.trim()).map(|item| (parse_value(item), item.to_string())).collect()
}

fn json_text(json: &Json) -> String {
    match json {
        Json::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn actual_text(actual: &Actual) -> String {
    let text = match actual {
        Actual::Undefined => "undefined".to_string(),
        Actual::Json(json) => json.to_string(),
    };
    if text.chars().count() > ACTUAL_CAP {
        let cut: String = text.chars().take(ACTUAL_CAP).collect();
        format!("{cut}…")
    } else {
        text
    }
}

/// `String(actual)` input for the regex.
fn regex_subject(actual: &Actual) -> String {
    match actual {
        Actual::Undefined => "undefined".to_string(),
        Actual::Json(Json::String(s)) => s.clone(),
        Actual::Json(other) => other.to_string(),
    }
}

// ----------------------------------------------------------------------------------------
// Operators

enum Verdict {
    Pass,
    /// Failed; the optional note is appended to the message.
    Fail(Option<String>),
}

fn verdict(pass: bool) -> Verdict {
    if pass { Verdict::Pass } else { Verdict::Fail(None) }
}

fn not_a_number() -> Verdict {
    Verdict::Fail(Some("expected a number".to_string()))
}

fn check<'js>(ctx: &Ctx<'js>, op: Op, actual: &Actual, raw: &str) -> Result<Verdict> {
    let parsed = parse_value(raw);
    let v = match op {
        Op::Eq => verdict(eq_value(actual, &parsed, raw)),
        Op::Neq => verdict(!eq_value(actual, &parsed, raw)),
        Op::Gt | Op::Gte | Op::Lt | Op::Lte => match (num_of(actual), num_of(&parsed)) {
            (Some(a), Some(b)) => verdict(match op {
                Op::Gt => a > b,
                Op::Gte => a >= b,
                Op::Lt => a < b,
                _ => a <= b,
            }),
            _ => not_a_number(),
        },
        Op::In | Op::NotIn => {
            let found = candidates(&parsed, raw).iter().any(|(candidate, text)| eq_value(actual, candidate, text));
            verdict(found == (op == Op::In))
        }
        Op::Contains | Op::NotContains => {
            let found = match actual {
                Actual::Json(Json::String(s)) => Some(s.contains(raw)),
                Actual::Json(Json::Array(items)) => Some(items.iter().any(|item| eq_value(&Actual::Json(item.clone()), &parsed, raw))),
                _ => None,
            };
            match found {
                Some(found) => verdict(found == (op == Op::Contains)),
                None => Verdict::Fail(Some("expected a string or an array".to_string())),
            }
        }
        Op::StartsWith | Op::EndsWith => match actual {
            Actual::Json(Json::String(s)) => verdict(if op == Op::StartsWith { s.starts_with(raw) } else { s.ends_with(raw) }),
            _ => Verdict::Fail(Some("expected a string".to_string())),
        },
        Op::Matches | Op::NotMatches => match regex_test(ctx, raw, &regex_subject(actual))? {
            Ok(found) => verdict(found == (op == Op::Matches)),
            Err(message) => Verdict::Fail(Some(message)),
        },
        Op::Between => {
            let inner = raw.trim().trim_start_matches('[').trim_end_matches(']');
            let bounds: Vec<Option<f64>> = inner.split(',').map(|part| part.trim().parse::<f64>().ok()).collect();
            match (num_of(actual), bounds.as_slice()) {
                (Some(x), [Some(lo), Some(hi)]) => verdict(*lo <= x && x <= *hi),
                _ => Verdict::Fail(Some("expected a number and a value like `1, 10`".to_string())),
            }
        }
        Op::Length => match num_of(&parsed) {
            Some(wanted) => {
                let len = match actual {
                    Actual::Json(Json::String(s)) => Some(s.chars().count()),
                    Actual::Json(Json::Array(a)) => Some(a.len()),
                    Actual::Json(Json::Object(o)) => Some(o.len()),
                    _ => None,
                };
                verdict(len.is_some_and(|len| len as f64 == wanted))
            }
            None => not_a_number(),
        },
        Op::IsEmpty => verdict(is_empty(actual)),
        Op::IsNotEmpty => verdict(!matches!(actual, Actual::Undefined | Actual::Json(Json::Null)) && !is_empty(actual)),
        Op::IsNull => verdict(matches!(actual, Actual::Json(Json::Null))),
        Op::IsUndefined => verdict(matches!(actual, Actual::Undefined)),
        Op::IsDefined => verdict(!matches!(actual, Actual::Undefined)),
        Op::IsTruthy => verdict(truthy(actual)),
        Op::IsFalsy => verdict(!truthy(actual)),
        Op::IsNumber => verdict(matches!(actual, Actual::Json(Json::Number(_)))),
        Op::IsString => verdict(matches!(actual, Actual::Json(Json::String(_)))),
        Op::IsBoolean => verdict(matches!(actual, Actual::Json(Json::Bool(_)))),
        Op::IsArray => verdict(matches!(actual, Actual::Json(Json::Array(_)))),
        Op::IsJson => verdict(matches!(actual, Actual::Json(Json::Array(_) | Json::Object(_)))),
    };
    Ok(v)
}

/// `new RegExp(src, flags).test(subject)` through the JS constructor, arguments passed as
/// values. The inner `Err` is the JS message of an invalid pattern.
fn regex_test<'js>(ctx: &Ctx<'js>, raw: &str, subject: &str) -> Result<std::result::Result<bool, String>> {
    let (source, flags) = match raw.strip_prefix('/').and_then(|rest| rest.rsplit_once('/')) {
        Some((source, flags)) if flags.chars().all(|c| c.is_ascii_alphabetic()) => (source, flags),
        _ => (raw, ""),
    };
    let constructor: Constructor = ctx.globals().get("RegExp")?;
    let regex = match constructor.construct::<_, Object>((source.to_string(), flags.to_string())) {
        Ok(regex) => regex,
        Err(rquickjs::Error::Exception) => return caught_message(ctx).map(Err),
        Err(other) => return Ok(Err(other.to_string())),
    };
    let test: Function = regex.get("test")?;
    match test.call::<_, bool>((This(regex.clone()), subject.to_string())) {
        Ok(found) => Ok(Ok(found)),
        Err(rquickjs::Error::Exception) => caught_message(ctx).map(Err),
        Err(other) => Ok(Err(other.to_string())),
    }
}

fn caught_message<'js>(ctx: &Ctx<'js>) -> Result<String> {
    let value = ctx.catch();
    if value.is_uncatchable_error() {
        return Err(ctx.throw(value));
    }
    Ok(error_message(&value))
}
