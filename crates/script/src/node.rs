//! Node-style globals built as Rust closures: `Buffer`, an asset-backed CommonJS `require`
//! and the `fs`, `path`, `buffer` and `crypto` shims it serves.
//!
//! The virtual file system is the collection's Assets folder, rooted at `/`. Scripts can only
//! read it (through [`Host::read_asset`]); every write function is installed as a thrower.

use std::rc::Rc;

use base64::Engine as _;
use base64::alphabet;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD, URL_SAFE_NO_PAD};
use rquickjs::context::EvalOptions;
use rquickjs::function::{IntoJsFunc, Opt, Rest, This};
use rquickjs::object::Property;
use rquickjs::{Ctx, Function, Object, Result, Value};

use crate::pm::{Shared, def, throw, with_state};

/// Most modules one run may load through `require`.
const MAX_MODULES: usize = 64;
/// Largest module source accepted.
const MAX_MODULE_BYTES: usize = 8 << 20;
/// Hidden JS-side module cache and built-in table.
const MODULES: &str = "__pgModules";

// ----------------------------------------------------------------------------------------
// Encodings

#[derive(Clone, Copy)]
enum Enc {
    Utf8,
    Base64,
    Base64Url,
    Hex,
    Latin1,
}

fn parse_enc<'js>(ctx: &Ctx<'js>, name: &str) -> Result<Enc> {
    match name.to_ascii_lowercase().as_str() {
        "utf8" | "utf-8" => Ok(Enc::Utf8),
        "base64" => Ok(Enc::Base64),
        "base64url" => Ok(Enc::Base64Url),
        "hex" => Ok(Enc::Hex),
        "latin1" | "binary" => Ok(Enc::Latin1),
        _ => Err(throw(ctx, &format!("Buffer encoding '{name}' is not supported in Postguy scripts"))),
    }
}

/// An optional encoding argument; `undefined` and `null` mean the default.
fn enc_arg<'js>(ctx: &Ctx<'js>, value: Option<Value<'js>>) -> Result<Enc> {
    match value {
        Some(v) if !v.is_undefined() && !v.is_null() => match v.as_string() {
            Some(s) => parse_enc(ctx, &s.to_string()?),
            None => Err(throw(ctx, "the encoding must be a string")),
        },
        _ => Ok(Enc::Utf8),
    }
}

fn encode(bytes: &[u8], enc: Enc) -> String {
    match enc {
        Enc::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        Enc::Base64 => STANDARD.encode(bytes),
        Enc::Base64Url => URL_SAFE_NO_PAD.encode(bytes),
        Enc::Hex => {
            let mut out = String::with_capacity(bytes.len().saturating_mul(2));
            for b in bytes {
                out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
                out.push(char::from_digit(u32::from(b & 0xf), 16).unwrap_or('0'));
            }
            out
        }
        Enc::Latin1 => bytes.iter().map(|b| char::from(*b)).collect(),
    }
}

/// Lenient base64: both alphabets, padding optional, stray characters ignored (as Node does).
fn decode_base64(text: &str) -> Vec<u8> {
    let cleaned: String = text
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '-' | '_'))
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    let engine = GeneralPurpose::new(&alphabet::STANDARD, GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent));
    engine.decode(cleaned).unwrap_or_default()
}

fn decode_hex(text: &str) -> Vec<u8> {
    let digits: Vec<u32> = text.chars().map_while(|c| c.to_digit(16)).collect();
    digits.chunks_exact(2).map(|pair| (pair.first().copied().unwrap_or(0) * 16 + pair.get(1).copied().unwrap_or(0)) as u8).collect()
}

fn decode_text(text: &str, enc: Enc) -> Vec<u8> {
    match enc {
        Enc::Utf8 => text.as_bytes().to_vec(),
        Enc::Base64 | Enc::Base64Url => decode_base64(text),
        Enc::Hex => decode_hex(text),
        Enc::Latin1 => text.chars().map(|c| (u32::from(c) & 0xff) as u8).collect(),
    }
}

// ----------------------------------------------------------------------------------------
// Buffer

/// A non-enumerable function property.
fn method<'js, P, F>(ctx: &Ctx<'js>, object: &Object<'js>, name: &str, f: F) -> Result<()>
where
    F: IntoJsFunc<'js, P> + 'js,
{
    let function = Function::new(ctx.clone(), f)?.with_name(name)?;
    object.prop(name, Property::from(function))
}

/// A Buffer over `bytes`. The bytes live in the closures, so they are freed with the object.
pub(crate) fn make_buffer<'js>(ctx: &Ctx<'js>, bytes: Rc<[u8]>) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    object.prop("__pgBuffer", Property::from(true))?;
    object.prop("length", Property::from(bytes.len() as f64))?;
    {
        let bytes = bytes.clone();
        method(ctx, &object, "toString", move |ctx: Ctx<'js>, enc: Opt<Value<'js>>| -> Result<String> { Ok(encode(&bytes, enc_arg(&ctx, enc.0)?)) })?;
    }
    {
        let bytes = bytes.clone();
        method(ctx, &object, "__pgB64", move || -> String { STANDARD.encode(&*bytes) })?;
    }
    Ok(object)
}

fn buffer_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>> {
    let buffer = Object::new(ctx.clone())?;
    def(ctx, &buffer, "from", move |ctx: Ctx<'js>, value: Value<'js>, enc: Opt<Value<'js>>| -> Result<Object<'js>> {
        let bytes: Vec<u8> = if let Some(text) = value.as_string() {
            decode_text(&text.to_string()?, enc_arg(&ctx, enc.0)?)
        } else if let Some(array) = value.as_array() {
            let mut out = Vec::with_capacity(array.len());
            for item in array.iter::<f64>() {
                let n = item.unwrap_or(0.0);
                out.push(if n.is_finite() { (n as i64 & 0xff) as u8 } else { 0 });
            }
            out
        } else if let Some(object) = value.as_object()
            && object.get::<_, Value>("__pgBuffer").is_ok_and(|v| v.as_bool() == Some(true))
        {
            let to_base64: Function = object.get("__pgB64")?;
            let text: String = to_base64.call((This(object.clone()),))?;
            decode_base64(&text)
        } else {
            return Err(throw(&ctx, "Buffer.from expects a string, an array of bytes or a Buffer"));
        };
        make_buffer(&ctx, Rc::from(bytes))
    })?;
    def(ctx, &buffer, "isBuffer", move |value: Value<'js>| -> bool {
        value.as_object().is_some_and(|o| o.get::<_, Value>("__pgBuffer").is_ok_and(|v| v.as_bool() == Some(true)))
    })?;
    Ok(buffer)
}

// ----------------------------------------------------------------------------------------
// Paths

/// `p` against the absolute directory `base`, as an asset path (no leading `/`). A path that
/// climbs above `/` is an error.
fn vfs_asset_path(base: &str, p: &str) -> std::result::Result<String, String> {
    let full = if p.starts_with('/') { p.to_string() } else { format!("{base}/{p}") };
    let mut stack: Vec<&str> = Vec::new();
    for segment in full.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return Err(format!("path {p} leaves the collection assets"));
                }
            }
            other => stack.push(other),
        }
    }
    Ok(stack.join("/"))
}

/// POSIX `path.normalize`.
fn normalize(p: &str) -> String {
    if p.is_empty() {
        return ".".to_string();
    }
    let absolute = p.starts_with('/');
    let trailing = p.ends_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for segment in p.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if stack.last().is_some_and(|last| *last != "..") {
                    stack.pop();
                } else if !absolute {
                    stack.push("..");
                }
            }
            other => stack.push(other),
        }
    }
    let mut out = stack.join("/");
    if out.is_empty() {
        return if absolute {
            "/".to_string()
        } else if trailing {
            "./".to_string()
        } else {
            ".".to_string()
        };
    }
    if trailing {
        out.push('/');
    }
    if absolute { format!("/{out}") } else { out }
}

fn join(parts: &[String]) -> String {
    let joined = parts.iter().filter(|p| !p.is_empty()).map(String::as_str).collect::<Vec<_>>().join("/");
    normalize(&joined)
}

/// `path.resolve` with `/` as the working directory.
fn resolve(parts: &[String]) -> String {
    let mut acc = String::new();
    for part in parts.iter().rev() {
        if part.is_empty() {
            continue;
        }
        acc = if acc.is_empty() { part.clone() } else { format!("{part}/{acc}") };
        if part.starts_with('/') {
            break;
        }
    }
    let absolute = if acc.starts_with('/') { acc } else { format!("/{acc}") };
    let normalized = normalize(&absolute);
    if normalized.len() > 1 { normalized.trim_end_matches('/').to_string() } else { normalized }
}

fn dirname(p: &str) -> String {
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        return if p.starts_with('/') { "/".to_string() } else { ".".to_string() };
    }
    match trimmed.rfind('/') {
        None => ".".to_string(),
        Some(0) => "/".to_string(),
        Some(i) => trimmed.get(..i).unwrap_or("").trim_end_matches('/').to_string(),
    }
}

fn basename(p: &str, ext: Option<&str>) -> String {
    let trimmed = p.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or("");
    match ext {
        Some(ext) if !ext.is_empty() && name != ext && name.ends_with(ext) => name.get(..name.len() - ext.len()).unwrap_or(name).to_string(),
        _ => name.to_string(),
    }
}

fn extname(p: &str) -> String {
    let name = basename(p, None);
    match name.rfind('.') {
        Some(i) if i > 0 => name.get(i..).unwrap_or("").to_string(),
        _ => String::new(),
    }
}

fn path_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>> {
    let path = Object::new(ctx.clone())?;
    def(ctx, &path, "join", move |parts: Rest<String>| -> String { join(&parts.0) })?;
    def(ctx, &path, "resolve", move |parts: Rest<String>| -> String { resolve(&parts.0) })?;
    def(ctx, &path, "normalize", move |p: String| -> String { normalize(&p) })?;
    def(ctx, &path, "dirname", move |p: String| -> String { dirname(&p) })?;
    def(ctx, &path, "basename", move |p: String, ext: Opt<String>| -> String { basename(&p, ext.0.as_deref()) })?;
    def(ctx, &path, "extname", move |p: String| -> String { extname(&p) })?;
    def(ctx, &path, "isAbsolute", move |p: String| -> bool { p.starts_with('/') })?;
    path.set("sep", "/")?;
    Ok(path)
}

// ----------------------------------------------------------------------------------------
// Assets through the host

fn rel_path<'js>(ctx: &Ctx<'js>, base: &str, p: &str) -> Result<String> {
    vfs_asset_path(base, p).map_err(|message| throw(ctx, &message))
}

/// Bytes of the asset at `p` (relative to the asset root, `/` allowed).
pub(crate) fn read_asset<'js>(ctx: &Ctx<'js>, shared: &Shared, p: &str) -> Result<Vec<u8>> {
    let rel = rel_path(ctx, "/", p)?;
    let host = with_state(ctx, shared, |s| s.host.clone())?;
    host.read_asset(&rel).map_err(|message| throw(ctx, &message))
}

/// Direct children of the asset folder `dir`.
pub(crate) fn list_assets<'js>(ctx: &Ctx<'js>, shared: &Shared, dir: &str) -> Result<Vec<String>> {
    let rel = rel_path(ctx, "/", dir)?;
    let host = with_state(ctx, shared, |s| s.host.clone())?;
    host.list_assets(&rel).map_err(|message| throw(ctx, &message))
}

/// Whether a file or folder exists; any host error means no.
pub(crate) fn asset_exists<'js>(ctx: &Ctx<'js>, shared: &Shared, p: &str) -> Result<bool> {
    let rel = rel_path(ctx, "/", p)?;
    if rel.is_empty() {
        return Ok(true);
    }
    let (parent, name) = rel.rsplit_once('/').unwrap_or(("", rel.as_str()));
    let host = with_state(ctx, shared, |s| s.host.clone())?;
    Ok(host.list_assets(parent).is_ok_and(|names| names.iter().any(|n| n == name)))
}

// ----------------------------------------------------------------------------------------
// fs and crypto

/// `fs` functions that would write; installed so the failure is actionable, not a TypeError.
const FS_WRITERS: [&str; 28] = [
    "writeFileSync",
    "appendFileSync",
    "mkdirSync",
    "unlinkSync",
    "rmSync",
    "rmdirSync",
    "renameSync",
    "copyFileSync",
    "truncateSync",
    "chmodSync",
    "symlinkSync",
    "openSync",
    "writeSync",
    "createWriteStream",
    "writeFile",
    "appendFile",
    "mkdir",
    "unlink",
    "rm",
    "rmdir",
    "rename",
    "copyFile",
    "truncate",
    "chmod",
    "symlink",
    "open",
    "write",
    "mkdtempSync",
];

fn fs_object<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<Object<'js>> {
    let fs = Object::new(ctx.clone())?;
    {
        let shared = shared.clone();
        def(ctx, &fs, "readFileSync", move |ctx: Ctx<'js>, p: String, options: Opt<Value<'js>>| -> Result<Value<'js>> {
            let encoding = match options.0 {
                Some(v) if v.is_string() => Some(v),
                Some(v) => match v.as_object() {
                    Some(o) => Some(o.get::<_, Value>("encoding")?),
                    None => None,
                },
                None => None,
            }
            .filter(|v| !v.is_undefined() && !v.is_null());
            let bytes = read_asset(&ctx, &shared, &p)?;
            match encoding {
                Some(v) => {
                    let enc = enc_arg(&ctx, Some(v))?;
                    Ok(rquickjs::String::from_str(ctx.clone(), &encode(&bytes, enc))?.into_value())
                }
                None => Ok(make_buffer(&ctx, Rc::from(bytes))?.into_value()),
            }
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &fs, "existsSync", move |ctx: Ctx<'js>, p: Opt<Value<'js>>| -> Result<bool> {
            match p.0.as_ref().and_then(|v| v.as_string()) {
                Some(s) => Ok(asset_exists(&ctx, &shared, &s.to_string()?).unwrap_or(false)),
                None => Ok(false),
            }
        })?;
    }
    {
        let shared = shared.clone();
        def(ctx, &fs, "readdirSync", move |ctx: Ctx<'js>, p: Opt<String>| -> Result<Vec<String>> {
            list_assets(&ctx, &shared, &p.0.unwrap_or_default())
        })?;
    }
    for name in FS_WRITERS {
        let message = format!("fs.{name} is not available in Postguy scripts (scripts can only read collection assets)");
        def(ctx, &fs, name, move |ctx: Ctx<'js>, _args: Rest<Value<'js>>| -> Result<()> { Err(throw(&ctx, &message)) })?;
    }
    Ok(fs)
}

fn crypto_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>> {
    let crypto = Object::new(ctx.clone())?;
    def(ctx, &crypto, "randomUUID", move || -> String { uuid::Uuid::new_v4().to_string() })?;
    Ok(crypto)
}

// ----------------------------------------------------------------------------------------
// require

fn is_path_spec(spec: &str) -> bool {
    spec.starts_with("./") || spec.starts_with("../") || spec.starts_with('/') || spec == "." || spec == ".."
}

fn make_require<'js>(ctx: &Ctx<'js>, shared: &Shared, dirname: String) -> Result<Function<'js>> {
    let shared = shared.clone();
    Function::new(ctx.clone(), move |ctx: Ctx<'js>, spec: String| -> Result<Value<'js>> { require_in(&ctx, &shared, &dirname, &spec) })?
        .with_name("require")
}

fn require_in<'js>(ctx: &Ctx<'js>, shared: &Shared, dirname: &str, spec: &str) -> Result<Value<'js>> {
    let modules: Object = ctx.globals().get(MODULES)?;
    let builtin = spec.strip_prefix("node:").unwrap_or(spec);
    if matches!(builtin, "fs" | "path" | "buffer" | "crypto") {
        return modules.get(builtin);
    }
    if !is_path_spec(spec) {
        let hint = if spec == "axios" { "; use pg.sendRequest" } else { "" };
        return Err(throw(ctx, &format!("require('{spec}') is not available in Postguy scripts; npm packages are not supported{hint}")));
    }
    let rel = rel_path(ctx, dirname, spec)?;
    let candidates = if rel.is_empty() {
        vec!["index.js".to_string()]
    } else {
        vec![rel.clone(), format!("{rel}.js"), format!("{rel}.json"), format!("{rel}/index.js")]
    };
    let host = with_state(ctx, shared, |s| s.host.clone())?;
    let mut first_error: Option<String> = None;
    for candidate in candidates {
        let key = format!("/{candidate}");
        if let Some(module) = modules.get::<_, Option<Object>>(key.as_str())? {
            return module.get("exports");
        }
        match host.read_asset(&candidate) {
            Ok(bytes) => return load_module(ctx, shared, &modules, &key, bytes),
            Err(message) => {
                first_error.get_or_insert(message);
            }
        }
    }
    let detail = first_error.unwrap_or_default();
    Err(throw(ctx, &format!("Cannot find module '{spec}' in the collection assets: {detail}")))
}

fn load_module<'js>(ctx: &Ctx<'js>, shared: &Shared, modules: &Object<'js>, key: &str, bytes: Vec<u8>) -> Result<Value<'js>> {
    let over = with_state(ctx, shared, |s| {
        if s.modules_loaded >= MAX_MODULES {
            return true;
        }
        s.modules_loaded = s.modules_loaded.saturating_add(1);
        false
    })?;
    if over {
        return Err(throw(ctx, &format!("scripts may load at most {MAX_MODULES} modules per run")));
    }
    if bytes.len() > MAX_MODULE_BYTES {
        return Err(throw(ctx, &format!("module {key} is larger than {} MiB", MAX_MODULE_BYTES >> 20)));
    }
    let text = String::from_utf8(bytes).map_err(|_| throw(ctx, &format!("module {key} is not UTF-8 text")))?;
    let source = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let module = Object::new(ctx.clone())?;
    module.set("id", key)?;
    module.set("filename", key)?;
    if key.ends_with(".json") {
        let value = ctx.json_parse(source)?;
        module.set("exports", value.clone())?;
        modules.set(key, module)?;
        return Ok(value);
    }
    let exports = Object::new(ctx.clone())?;
    module.set("exports", exports.clone())?;
    // Cached before it runs, so a dependency cycle sees the partial `module.exports`.
    modules.set(key, module.clone())?;
    let dir = dirname(key);
    let outcome = (|| -> Result<()> {
        let wrapped = format!("(function(exports, require, module, __filename, __dirname){{{source}\n}})");
        let mut options = EvalOptions::default();
        options.global = true;
        options.strict = false;
        options.filename = Some(key.to_string());
        let function: Function = ctx.eval_with_options(wrapped, options)?;
        let require = make_require(ctx, shared, dir.clone())?;
        function.call::<_, Value>((This(exports.clone()), exports, require, module.clone(), key, dir))?;
        Ok(())
    })();
    if let Err(err) = outcome {
        // A module that failed to load is not cached (Node semantics).
        let _ = modules.remove(key);
        return Err(err);
    }
    module.get("exports")
}

/// Install `Buffer`, `require`, `__dirname`, `__filename` and the module table.
pub(crate) fn install<'js>(ctx: &Ctx<'js>, shared: &Shared) -> Result<()> {
    let globals = ctx.globals();
    let buffer = buffer_object(ctx)?;
    let modules = Object::new(ctx.clone())?;
    modules.set("fs", fs_object(ctx, shared)?)?;
    modules.set("path", path_object(ctx)?)?;
    modules.set("crypto", crypto_object(ctx)?)?;
    let buffer_module = Object::new(ctx.clone())?;
    buffer_module.set("Buffer", buffer.clone())?;
    modules.set("buffer", buffer_module)?;
    globals.prop(MODULES, Property::from(modules))?;
    globals.set("Buffer", buffer)?;
    globals.set("__dirname", "/")?;
    globals.set("__filename", "/script.js")?;
    globals.set("require", make_require(ctx, shared, "/".to_string())?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_path_helpers() {
        assert_eq!(normalize("/a/b/../c/./d/"), "/a/c/d/");
        assert_eq!(normalize("../x"), "../x");
        assert_eq!(normalize(""), ".");
        assert_eq!(join(&["/lib".to_string(), "images/a.png".to_string()]), "/lib/images/a.png");
        assert_eq!(resolve(&["a".to_string(), "../b".to_string()]), "/b");
        assert_eq!(dirname("/lib/utils.js"), "/lib");
        assert_eq!(dirname("/x"), "/");
        assert_eq!(basename("/a/b.txt", Some(".txt")), "b");
        assert_eq!(extname("a/.bashrc"), "");
        assert_eq!(extname("a/b.tar.gz"), ".gz");
    }

    #[test]
    fn vfs_refuses_to_leave_the_root() {
        assert_eq!(vfs_asset_path("/lib", "../images/a.png"), Ok("images/a.png".to_string()));
        assert!(vfs_asset_path("/lib", "../../x").is_err());
    }

    #[test]
    fn lenient_base64_and_hex() {
        assert_eq!(decode_base64("e30"), b"{}");
        assert_eq!(decode_base64("e30="), b"{}");
        assert_eq!(decode_hex("6a6B"), b"jk");
    }
}
