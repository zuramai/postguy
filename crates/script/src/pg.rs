//! The `pg` global: every `pm` member (the same JS values), the Bruno-style variable and
//! request methods, flow control, `pg.assets` and `pg.report`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rquickjs::function::Opt;
use rquickjs::{Ctx, Object, Result, Type, Value};
use serde_json::Value as Json;

use crate::bru;
use crate::convert;
use crate::node;
use crate::pm::{Shared, def, throw, with_state};

/// Most fields `pg.report` holds.
const REPORT_KEYS: usize = 100;
/// Longest report key, in characters.
const REPORT_KEY_CHARS: usize = 128;
/// Longest report value, in bytes.
const REPORT_VALUE_BYTES: usize = 32 * 1024;

fn truncate_value(mut text: String) -> String {
    if text.len() > REPORT_VALUE_BYTES {
        let mut cut = REPORT_VALUE_BYTES;
        while !text.is_char_boundary(cut) {
            cut = cut.saturating_sub(1);
        }
        text.truncate(cut);
        text.push('…');
    }
    text
}

/// A report cell: primitives as JSON, objects as their `JSON.stringify` text, `undefined` none.
fn report_json<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Option<Json>> {
    Ok(match value.type_of() {
        Type::Uninitialized | Type::Undefined | Type::Function | Type::Constructor | Type::Symbol | Type::BigInt => None,
        Type::Null => Some(Json::Null),
        Type::Bool => Some(Json::Bool(value.as_bool().unwrap_or(false))),
        Type::Int | Type::Float => Some(convert::number_json(value.as_number().unwrap_or(f64::NAN))),
        Type::String => Some(Json::String(truncate_value(value.as_string().map(|s| s.to_string()).transpose()?.unwrap_or_default()))),
        _ => ctx.json_stringify(value.clone())?.map(|s| s.to_string()).transpose()?.map(|text| Json::String(truncate_value(text))),
    })
}

fn assets_object<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Object<'js>> {
    let assets = Object::new(ctx.clone())?;
    {
        let shared = shared.clone();
        def(ctx, &assets, "base64", move |ctx: Ctx<'js>, path: String| -> Result<String> {
            Ok(STANDARD.encode(node::read_asset(&ctx, &shared, &path)?))
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &assets, "text", move |ctx: Ctx<'js>, path: String| -> Result<String> {
            let bytes = node::read_asset(&ctx, &shared, &path)?;
            String::from_utf8(bytes).map_err(|_| throw(&ctx, &format!("asset {path} is not UTF-8 text; use pg.assets.base64")))
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &assets, "exists", move |ctx: Ctx<'js>, path: String| -> Result<bool> { node::asset_exists(&ctx, &shared, &path) })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &assets, "list", move |ctx: Ctx<'js>, dir: Opt<String>| -> Result<Vec<String>> {
            node::list_assets(&ctx, &shared, &dir.0.unwrap_or_default())
        })?;
    }
    Ok(assets)
}

fn report_object<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Object<'js>> {
    let report = Object::new(ctx.clone())?;
    {
        let shared = shared.clone();
        def(ctx, &report, "set", move |ctx: Ctx<'js>, key: String, value: Value<'js>| -> Result<()> {
            if key.chars().count() > REPORT_KEY_CHARS {
                return Err(throw(&ctx, &format!("pg.report keys may be at most {REPORT_KEY_CHARS} characters")));
            }
            let json = report_json(&ctx, &value)?;
            let full = with_state(&ctx, &shared, |s| {
                match json {
                    None => {
                        s.report.shift_remove(&key);
                    }
                    Some(json) => {
                        if !s.report.contains_key(&key) && s.report.len() >= REPORT_KEYS {
                            return true;
                        }
                        s.report.insert(key, json);
                    }
                }
                false
            })?;
            if full {
                return Err(throw(&ctx, &format!("pg.report holds at most {REPORT_KEYS} fields")));
            }
            Ok(())
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &report, "get", move |ctx: Ctx<'js>, key: String| -> Result<Value<'js>> {
            match with_state(&ctx, &shared, |s| s.report.get(&key).cloned())? {
                Some(json) => convert::json_to_js(&ctx, &json),
                None => Ok(Value::new_undefined(ctx.clone())),
            }
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &report, "toObject", move |ctx: Ctx<'js>| -> Result<Value<'js>> {
            let map = with_state(&ctx, &shared, |s| s.report.clone())?;
            convert::json_to_js(&ctx, &Json::Object(map))
        })?;
    }
    Ok(report)
}

/// Install `pg` once `pm` exists.
pub(crate) fn install<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<()> {
    let globals = ctx.globals();
    let pm: Object = globals.get("pm")?;
    let pg = Object::new(ctx.clone())?;
    for entry in pm.props::<String, Value>() {
        let (key, value) = entry?;
        pg.set(key, value)?;
    }

    let request = crate::pm::request_object(ctx, shared)?;
    bru::install_request_methods(ctx, &request, shared, "pg.request")?;
    pg.set("request", request)?;

    bru::install_var_methods(ctx, &pg, shared)?;

    let execution: Object = pm.get("execution")?;
    pg.set("setNextRequest", execution.get::<_, Value>("setNextRequest")?)?;
    pg.set("skipRequest", execution.get::<_, Value>("skipRequest")?)?;
    {
        let shared = shared.clone();
        def(ctx, &pg, "stopExecution", move |ctx: Ctx<'js>| -> Result<()> {
            with_state(&ctx, &shared, |s| s.next = postguy_model::NextRequest::Stop)
        })?;
    }

    pg.set("assets", assets_object(ctx, shared)?)?;
    pg.set("report", report_object(ctx, shared)?)?;
    globals.set("pg", pg)
}
