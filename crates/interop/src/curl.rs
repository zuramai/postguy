//! cURL import/export: [`parse_curl`] turns a pasted cURL command line
//! into a [`ParsedCurl`] (an [`HttpRequest`] plus cURL-only out-of-band data), and
//! [`generate_curl`] turns a request back into Postman-style cURL text. The CLI's curl import
//! and the code-snippet panel share this one implementation.
//!
//! # Input handling
//!
//! The crate works on `&str` only — the caller reads files (with the `postguy-util` capped
//! readers) and passes the text in; [`parse_curl`] additionally refuses inputs over
//! [`INPUT_CAP`] bytes and caps tokens per command, so hostile input degrades to
//! [`CurlError`]s and warnings, never panics.
//!
//! # Parsing decisions
//!
//! - **Multiple `-d` pieces** (and `--data`, `--data-raw`, `--data-ascii`,
//!   `--data-urlencode`, `--data-binary`): concatenated **without any separator**, exactly as
//!   cURL's man page specifies ("if you use --data more than once, the data pieces are
//!   concatenated"). We deliberately do not insert `&` or `\n`; that would corrupt the body
//!   the user actually meant to send.
//! - **Multiple commands in one input**: split when a logical line (after `\` and `^`
//!   continuations are joined) starts with `curl`. Anything else is reported as a warning and
//!   skipped.
//! - **Unknown flags** are reported as warnings, never fatal.
//! - **`-b @file`** reads the file at parse time (capped); **`-b @-`** (stdin) is an
//!   [`CurlError::Unsupported`] because a pure parser cannot consume stdin.
//! - **`--data-urlencode`** is stored as a plain data piece; the URL-encoding it implies is a
//!   send-time concern of the HTTP engine, not of the model.
//! - **`--compressed`, `-L`, `-k`** and friends map onto [`RequestSettings`] where the model
//!   has a field; flags with no model equivalent are reported and ignored.
//! - **Query params are not split out of the URL** into `request.params`: the URL string is
//!   kept verbatim (lossless round trips beat UI convenience here; the UI syncs params from
//!   the URL itself).

use postguy_model::{AuthConfig, Body, HttpMethod, HttpRequest, HttpVersion, KeyValue, KeyValueKind, RawLanguage, RequestSettings};

use crate::error::{CurlError, Result};

/// Maximum input size accepted by [`parse_curl_from_str`]: 1 MiB of command line.
pub const INPUT_CAP: usize = 1024 * 1024;
/// Cap on a single token (a quoted string): 256 KiB of characters.
const MAX_TOKEN_CHARS: usize = 256 * 1024;
/// Cap on logical lines and on tokens per command.
const MAX_LINES: usize = 8192;
const MAX_TOKENS_PER_COMMAND: usize = 4096;
/// Cap on a cookie file read for `-b @file`.
const COOKIE_FILE_CAP: usize = 64 * 1024;
/// Warnings are capped so a hostile input cannot balloon the output.
const MAX_WARNINGS: usize = 200;

/// One parsed cURL command: the request, plus cURL-only data that has no
/// [`HttpRequest`] field.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCurl {
    pub request: HttpRequest,
    /// From `-x/--proxy`; the caller decides what to do with it (the model has no proxy field).
    pub proxy: Option<String>,
    /// Unsupported flags and recoverable oddities, in input order.
    pub warnings: Vec<String>,
}

/// Parse cURL command line text into one [`ParsedCurl`] per `curl` command found.
///
/// The input is already a `&str`: the caller reads files via `postguy_util::read_string_capped`
/// and passes the text. Inputs over [`INPUT_CAP`] bytes are rejected with
/// [`CurlError::TooLarge`]; an input with no `curl` command at all is [`CurlError::NoCommand`].
pub fn parse_curl(input: &str) -> Result<Vec<ParsedCurl>> {
    parse_curl_from_str(input)
}

/// Same as [`parse_curl`]; kept as the explicit "works on `&str` only" entry point.
pub fn parse_curl_from_str(input: &str) -> Result<Vec<ParsedCurl>> {
    if input.len() > INPUT_CAP {
        return Err(CurlError::TooLarge { what: "curl input".to_string(), size: input.len(), cap: INPUT_CAP });
    }
    let mut warnings = Vec::new();
    let lines = join_continuations(input, &mut warnings);
    let mut commands: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    for line in lines {
        let tokens = lex_line(&line, &mut warnings);
        if tokens.is_empty() {
            continue;
        }
        if is_curl_token(&tokens[0]) {
            commands.push((tokens, Vec::new()));
        } else {
            warn(&mut warnings, "line ignored: does not start with a `curl` command");
        }
    }
    if commands.is_empty() {
        return Err(CurlError::NoCommand);
    }
    let mut out = Vec::with_capacity(commands.len());
    for (tokens, _) in commands {
        match parse_command(&tokens, &mut warnings) {
            Ok(Some(parsed)) => out.push(parsed),
            Ok(None) => warn(&mut warnings, "command skipped: no URL given"),
            Err(err) => return Err(err),
        }
    }
    if out.is_empty() {
        return Err(CurlError::NoCommand);
    }
    for parsed in &mut out {
        for w in &warnings {
            if parsed.warnings.len() >= MAX_WARNINGS {
                break;
            }
            parsed.warnings.push(w.clone());
        }
    }
    Ok(out)
}

fn warn(warnings: &mut Vec<String>, message: &str) {
    if warnings.len() < MAX_WARNINGS {
        warnings.push(message.to_string());
    }
}

/// Whether a token starts a new cURL command (bare `curl` or a path ending in `/curl`).
fn is_curl_token(token: &str) -> bool {
    token == "curl" || token.ends_with("/curl") || token.ends_with("\\curl")
}

/// Join `\` (POSIX) and `^` (Windows cmd) line continuations into logical lines: a trailing
/// continuation character (ignoring trailing whitespace) merges the next physical line onto
/// this one with no separator.
fn join_continuations(input: &str, warnings: &mut Vec<String>) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    let mut open = false;
    for line in input.lines() {
        let trimmed = line.trim_end();
        let cont = trimmed.ends_with('\\') || trimmed.ends_with('^');
        let body = if cont { &trimmed[..trimmed.len() - 1] } else { line };
        if open {
            if let Some(last) = merged.last_mut() {
                last.push_str(body);
            }
        } else if merged.len() < MAX_LINES {
            merged.push(body.to_string());
        } else {
            warn(warnings, "input has too many lines; the rest was ignored");
            break;
        }
        open = cont;
    }
    merged
}

/// Shell-ish lexer for one logical line: single quotes, double quotes with `\"` escapes,
/// ANSI-C `$'...'` strings, unquoted backslash escapes, whitespace-separated tokens.
fn lex_line(line: &str, warnings: &mut Vec<String>) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quoted = false; // the token exists even when a quoted string is empty (`--data-raw ''`)
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == ' ' || c == '\t' {
            push_token(&mut tokens, &mut cur, quoted, warnings);
            quoted = false;
            i += 1;
        } else if c == '\'' {
            i = lex_single(&chars, i + 1, &mut cur, warnings);
            quoted = true;
        } else if c == '"' {
            i = lex_double(&chars, i + 1, &mut cur, warnings);
            quoted = true;
        } else if c == '$' && chars.get(i + 1) == Some(&'\'') {
            i = lex_ansi_c(&chars, i + 2, &mut cur, warnings);
            quoted = true;
        } else if c == '\\' {
            if let Some(&next) = chars.get(i + 1) {
                cur.push(next);
                i += 2;
            } else {
                i += 1;
            }
        } else {
            cur.push(c);
            i += 1;
        }
    }
    push_token(&mut tokens, &mut cur, quoted, warnings);
    tokens
}

fn push_token(tokens: &mut Vec<String>, cur: &mut String, quoted: bool, warnings: &mut Vec<String>) {
    if cur.is_empty() && !quoted {
        return;
    }
    if cur.chars().count() > MAX_TOKEN_CHARS {
        warn(warnings, "a quoted token exceeded the length cap and was truncated");
        let truncated: String = cur.chars().take(MAX_TOKEN_CHARS).collect();
        *cur = truncated;
    }
    tokens.push(std::mem::take(cur));
    if tokens.len() >= MAX_TOKENS_PER_COMMAND {
        warn(warnings, "too many tokens in one command; the rest was ignored");
    }
}

/// Single-quoted string: everything literal until the closing `'`.
fn lex_single(chars: &[char], start: usize, cur: &mut String, warnings: &mut Vec<String>) -> usize {
    let mut i = start;
    while i < chars.len() {
        if chars[i] == '\'' {
            return i + 1;
        }
        cur.push(chars[i]);
        i += 1;
    }
    warn(warnings, "unterminated single-quoted string; treated literally");
    chars.len()
}

/// Double-quoted string: backslash keeps meaning only before `$`, `` ` ``, `"`, `\`.
fn lex_double(chars: &[char], start: usize, cur: &mut String, warnings: &mut Vec<String>) -> usize {
    let mut i = start;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            return i + 1;
        }
        if c == '\\' {
            match chars.get(i + 1) {
                Some('"') => {
                    cur.push('"');
                    i += 2;
                }
                Some('\\') => {
                    cur.push('\\');
                    i += 2;
                }
                Some('$') => {
                    cur.push('$');
                    i += 2;
                }
                Some('`') => {
                    cur.push('`');
                    i += 2;
                }
                Some('\n') => {
                    i += 2; // line continuation inside quotes
                }
                _ => {
                    cur.push('\\');
                    i += 1;
                }
            }
        } else {
            cur.push(c);
            i += 1;
        }
    }
    warn(warnings, "unterminated double-quoted string; treated literally");
    chars.len()
}

/// ANSI-C `$'...'` string: the usual C escapes; unknown escapes keep the backslash.
fn lex_ansi_c(chars: &[char], start: usize, cur: &mut String, warnings: &mut Vec<String>) -> usize {
    let mut i = start;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            return i + 1;
        }
        if c == '\\' {
            i += 1;
            match chars.get(i) {
                Some('n') => cur.push('\n'),
                Some('t') => cur.push('\t'),
                Some('r') => cur.push('\r'),
                Some('f') => cur.push('\x0c'),
                Some('v') => cur.push('\x0b'),
                Some('b') => cur.push('\x08'),
                Some('a') => cur.push('\x07'),
                Some('e') => cur.push('\x1b'),
                Some(&esc) => {
                    cur.push('\\');
                    cur.push(esc);
                }
                None => {
                    warn(warnings, "unterminated ANSI-C string; treated literally");
                    return chars.len();
                }
            }
            i += 1;
        } else {
            cur.push(c);
            i += 1;
        }
    }
    warn(warnings, "unterminated ANSI-C string; treated literally");
    chars.len()
}

/// Mutable parse state for one cURL command.
struct Command {
    url: Option<String>,
    method: Option<HttpMethod>,
    headers: Vec<KeyValue>,
    data: Vec<String>,
    form: Vec<KeyValue>,
    get_mode: bool,
    insecure: bool,
    follow_redirects: bool,
    http_version: Option<HttpVersion>,
    proxy: Option<String>,
    max_redirs: Option<u32>,
    timeout_ms: Option<u64>,
    json: bool,
    basic: Option<(String, String)>,
}

impl Command {
    fn new() -> Self {
        Command {
            url: None,
            method: None,
            headers: Vec::new(),
            data: Vec::new(),
            form: Vec::new(),
            get_mode: false,
            insecure: false,
            follow_redirects: false,
            http_version: None,
            proxy: None,
            max_redirs: None,
            timeout_ms: None,
            json: false,
            basic: None,
        }
    }
}

/// Parse one `curl ...` command's tokens into a [`ParsedCurl`]; `Ok(None)` when no URL appeared.
fn parse_command(tokens: &[String], warnings: &mut Vec<String>) -> Result<Option<ParsedCurl>> {
    let mut cmd = Command::new();
    let mut i = 1; // tokens[0] is the `curl` program name
    while i < tokens.len() {
        let token = tokens[i].as_str();
        i += 1;
        if token == "--" {
            // Everything after `--` is positional; treat the next bare token as the URL.
            while i < tokens.len() {
                let rest = &tokens[i];
                i += 1;
                if cmd.url.is_none() {
                    cmd.url = Some(rest.clone());
                } else {
                    warn(warnings, "extra positional argument after the URL was ignored");
                }
            }
            break;
        }
        if token == "-" {
            warn(warnings, "unsupported flag - (ignored)");
            continue;
        }
        if !token.starts_with('-') {
            if cmd.url.is_none() {
                cmd.url = Some(token.to_string());
            } else {
                warn(warnings, "extra positional argument after the URL was ignored");
            }
            continue;
        }
        let (name, attached) = split_flag(token);
        match name.as_str() {
            "request" | "X" | "method" => {
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    cmd.method = parse_method(&value, warnings);
                }
            }
            "url" => {
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    set_url(&mut cmd, &value, warnings);
                }
            }
            "header" | "H" => {
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    push_header(&mut cmd.headers, &value, warnings);
                }
            }
            "user-agent" | "A" => {
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    upsert_header(&mut cmd.headers, "User-Agent", &value);
                }
            }
            "referer" | "e" => {
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    upsert_header(&mut cmd.headers, "Referer", &value);
                }
            }
            "cookie" | "b" => {
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    handle_cookie(&mut cmd.headers, &value)?;
                }
            }
            "data" | "d" | "data-raw" | "data-ascii" | "data-urlencode" | "data-binary" => {
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    cmd.data.push(value);
                }
            }
            "form" | "F" | "form-string" => {
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    push_form_row(&mut cmd.form, &value, warnings);
                }
            }
            "user" | "u" => {
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    cmd.basic = Some(split_user(&value));
                }
            }
            "json" => {
                cmd.json = true;
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    cmd.data.push(value);
                }
            }
            "get" | "G" => cmd.get_mode = true,
            "location" | "L" => cmd.follow_redirects = true,
            "insecure" | "k" => cmd.insecure = true,
            "head" | "I" => cmd.method = Some(HttpMethod::Head),
            "proxy" | "x" => {
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    cmd.proxy = Some(value);
                }
            }
            "max-redirs" => {
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    cmd.max_redirs = parse_u32(&value, "max-redirs", warnings);
                }
            }
            "max-time" | "m" => {
                if let Some(value) = take_value(&attached, tokens, &mut i, &name, warnings) {
                    cmd.timeout_ms = parse_timeout(&value, warnings);
                }
            }
            "http1.0" | "http1.1" => cmd.http_version = Some(HttpVersion::Http1),
            "http2" => cmd.http_version = Some(HttpVersion::Http2),
            "http2-prior-knowledge" => {
                cmd.http_version = Some(HttpVersion::Http2);
                warn(warnings, "flag --http2-prior-knowledge mapped to HTTP/2");
            }
            "compressed" => warn(warnings, "unsupported flag --compressed (ignored): the engine decides compression"),
            "output" | "o" | "cookie-jar" | "c" | "connect-timeout" | "retry" | "noproxy" | "certificate" | "E" | "key" | "cert" | "cacert"
            | "unix-socket" | "resolve" | "header-file" | "config" | "K" => {
                // Known value-taking flags with no model mapping: consume the value, report.
                let _ = take_value(&attached, tokens, &mut i, &name, warnings);
                warn(warnings, &format!("unsupported flag {} (ignored)", flag_label(&name)));
            }
            "silent" | "s" | "show-error" | "S" | "verbose" | "v" | "include" | "i" | "fail" | "no-buffer" | "progress-bar" | "#" | "ipv4" | "4"
            | "ipv6" | "6" | "no-keepalive" | "fail-with-body" | "no-progress-meter" => {}
            _ => {
                if attached.is_none() {
                    let _ = take_value(&attached, tokens, &mut i, &name, warnings);
                }
                warn(warnings, &format!("unsupported flag {} (ignored)", flag_label(&name)));
            }
        }
    }
    Ok(build_result(cmd, warnings))
}

fn flag_label(name: &str) -> String {
    format!("--{name}")
}

/// Strip leading dashes and split an attached value: `--flag=value` or `-Hvalue`
/// (the shell concatenates `-H'x: y'` into one token `-Hx: y`).
fn split_flag(token: &str) -> (String, Option<String>) {
    if let Some(stripped) = token.strip_prefix("--") {
        if let Some(eq) = stripped.find('=') {
            return (stripped[..eq].to_string(), Some(stripped[eq + 1..].to_string()));
        }
        return (stripped.to_string(), None);
    }
    let stripped = token.trim_start_matches('-');
    let mut chars = stripped.chars();
    match chars.next() {
        Some(first) => {
            let rest: String = chars.collect();
            (first.to_string(), if rest.is_empty() { None } else { Some(rest) })
        }
        None => (String::new(), None),
    }
}

/// Consume the value for a value-taking flag: the attached `=value`/`-Hvalue` part, or the
/// next token. Like cURL, the next token is consumed even if it starts with `-`; a missing
/// value is a warning, not an error.
fn take_value(attached: &Option<String>, tokens: &[String], i: &mut usize, name: &str, warnings: &mut Vec<String>) -> Option<String> {
    if let Some(value) = attached {
        return Some(value.clone());
    }
    if *i < tokens.len() {
        let value = tokens[*i].clone();
        *i += 1;
        return Some(value);
    }
    warn(warnings, &format!("flag {} without a value (ignored)", flag_label(name)));
    None
}

fn parse_method(value: &str, warnings: &mut Vec<String>) -> Option<HttpMethod> {
    let upper = value.trim().to_ascii_uppercase();
    let method = match upper.as_str() {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        "HEAD" => HttpMethod::Head,
        "OPTIONS" => HttpMethod::Options,
        other => {
            warn(warnings, &format!("unsupported method {other} (kept GET)"));
            return None;
        }
    };
    Some(method)
}

fn set_url(cmd: &mut Command, value: &str, warnings: &mut Vec<String>) {
    if cmd.url.is_some() {
        warn(warnings, "more than one URL given; the first one was kept");
        return;
    }
    cmd.url = Some(value.to_string());
}

fn push_header(headers: &mut Vec<KeyValue>, raw: &str, warnings: &mut Vec<String>) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }
    match trimmed.split_once(':') {
        Some((key, value)) => {
            let key = key.trim();
            if key.is_empty() {
                warn(warnings, "header with an empty name was ignored");
                return;
            }
            upsert_header(headers, key, value.trim());
        }
        None => warn(warnings, &format!("header `{trimmed}` without a colon was ignored")),
    }
}

/// Insert a header, replacing an earlier row with the same key (case-insensitive) so `-H`
/// overrides like cURL do.
fn upsert_header(headers: &mut Vec<KeyValue>, key: &str, value: &str) {
    for row in headers.iter_mut() {
        if row.key.eq_ignore_ascii_case(key) {
            row.value = value.to_string();
            return;
        }
    }
    headers.push(KeyValue { key: key.to_string(), value: value.to_string(), enabled: true, ..KeyValue::default() });
}

/// `-b`: `name=value[;name2=value2]` pairs become the `Cookie` header; `@file` reads the file
/// (capped); `@-` (stdin) is unsupported.
fn handle_cookie(headers: &mut Vec<KeyValue>, value: &str) -> Result<()> {
    if let Some(path) = value.strip_prefix('@') {
        if path == "-" {
            return Err(CurlError::Unsupported { what: "`-b @-` (reading cookies from stdin)".to_string() });
        }
        let text = postguy_util::read_string_capped(std::path::Path::new(path), COOKIE_FILE_CAP)
            .map_err(|source| CurlError::Read { what: format!("cookie file `{path}`"), source })?;
        append_cookie(headers, &text);
        Ok(())
    } else {
        append_cookie(headers, value);
        Ok(())
    }
}

fn append_cookie(headers: &mut Vec<KeyValue>, pairs: &str) {
    let joined = pairs.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("; ");
    if joined.is_empty() {
        return;
    }
    for row in headers.iter_mut() {
        if row.key.eq_ignore_ascii_case("Cookie") {
            if row.value.is_empty() {
                row.value = joined;
            } else {
                row.value.push_str("; ");
                row.value.push_str(&joined);
            }
            return;
        }
    }
    headers.push(KeyValue { key: "Cookie".to_string(), value: joined, enabled: true, ..KeyValue::default() });
}

/// `-F key=value` / `-F key=@path`: file rows keep the path with `KeyValueKind::File`.
fn push_form_row(form: &mut Vec<KeyValue>, raw: &str, warnings: &mut Vec<String>) {
    let Some((key, value)) = raw.split_once('=') else {
        warn(warnings, &format!("form field `{raw}` without `=` was ignored"));
        return;
    };
    if key.is_empty() {
        warn(warnings, "form field with an empty name was ignored");
        return;
    }
    let mut value = value.to_string();
    let mut kind = KeyValueKind::Text;
    if let Some(path) = value.strip_prefix('@') {
        kind = KeyValueKind::File;
        // `@path;type=mime` — the type suffix is not modelled.
        if let Some((path, _mime)) = path.split_once(';') {
            warn(warnings, "form field MIME type suffix is not modelled and was dropped");
            value = path.to_string();
        } else {
            value = path.to_string();
        }
    }
    form.push(KeyValue { key: key.to_string(), value, enabled: true, kind, ..KeyValue::default() });
}

/// `-u user[:password]`: Basic auth config carries the plaintext cURL semantics.
fn split_user(value: &str) -> (String, String) {
    match value.split_once(':') {
        Some((user, pass)) => (user.to_string(), pass.to_string()),
        None => (value.to_string(), String::new()),
    }
}

fn parse_u32(value: &str, what: &str, warnings: &mut Vec<String>) -> Option<u32> {
    match value.trim().parse::<u32>() {
        Ok(n) => Some(n),
        Err(_) => {
            warn(warnings, &format!("flag --{what} value `{value}` is not a number (ignored)"));
            None
        }
    }
}

fn parse_timeout(value: &str, warnings: &mut Vec<String>) -> Option<u64> {
    match value.trim().parse::<f64>() {
        Ok(secs) if secs.is_finite() && secs >= 0.0 => Some((secs * 1000.0).min((u64::MAX / 2) as f64).round() as u64),
        _ => {
            warn(warnings, &format!("flag --max-time value `{value}` is not a number of seconds (ignored)"));
            None
        }
    }
}

/// Assemble the [`HttpRequest`] from the parsed flags.
fn build_result(mut cmd: Command, warnings: &mut Vec<String>) -> Option<ParsedCurl> {
    let Some(url) = cmd.url else {
        warn(warnings, "command skipped: no URL given");
        return None;
    };
    let mut headers = std::mem::take(&mut cmd.headers);
    let mut url = url;
    let data = std::mem::take(&mut cmd.data);
    let mut body = Body::None;
    let has_form = !cmd.form.is_empty();

    if cmd.json && !headers.iter().any(|h| h.key.eq_ignore_ascii_case("Content-Type")) {
        upsert_header(&mut headers, "Content-Type", "application/json");
    }

    if !cmd.form.is_empty() {
        body = Body::FormData(std::mem::take(&mut cmd.form));
    } else if !data.is_empty() {
        // cURL concatenates multiple -d pieces with no separator (curl man page).
        let text = data.concat();
        if cmd.get_mode {
            let encoded = encode_query_component(&text);
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&encoded);
        } else {
            body = Body::Raw { language: raw_language_from_headers(&headers), text, extras: postguy_model::Extras::default() };
        }
    }

    // `-d`/`-F`/`--json` imply POST when no method was given (unless `-G` moved data to the
    // query string, which keeps the request a GET). All-empty `-d ''` pieces are kept as an
    // empty raw body but do not imply POST — with an explicit method that distinction is moot.
    let implied_post = cmd.json || (!cmd.get_mode && (data.iter().any(|piece| !piece.is_empty()) || has_form));
    let method = cmd.method.or(if implied_post { Some(HttpMethod::Post) } else { None });

    let mut settings = RequestSettings { verify_ssl: !cmd.insecure, follow_redirects: cmd.follow_redirects, ..RequestSettings::default() };
    if let Some(version) = cmd.http_version {
        settings.http_version = version;
    }
    if let Some(redirs) = cmd.max_redirs {
        settings.max_redirects = redirs;
    }
    if let Some(ms) = cmd.timeout_ms {
        settings.timeout_ms = ms;
    }

    let auth = match cmd.basic {
        Some((user, pass)) if !user.is_empty() || !pass.is_empty() => {
            AuthConfig::Basic(Box::new(postguy_model::auth::BasicAuth { username: user, password: pass, ..Default::default() }))
        }
        _ => AuthConfig::Inherit,
    };

    Some(ParsedCurl {
        request: HttpRequest { method: method.unwrap_or_default(), url, headers, body, auth, settings, ..HttpRequest::default() },
        proxy: cmd.proxy,
        warnings: Vec::new(),
    })
}

fn raw_language_from_headers(headers: &[KeyValue]) -> RawLanguage {
    for row in headers {
        if row.key.eq_ignore_ascii_case("Content-Type") {
            let ct = row.value.to_ascii_lowercase();
            if ct.contains("json") {
                return RawLanguage::Json;
            }
            if ct.contains("xml") {
                return RawLanguage::Xml;
            }
            if ct.contains("html") {
                return RawLanguage::Html;
            }
            if ct.contains("javascript") {
                return RawLanguage::JavaScript;
            }
        }
    }
    RawLanguage::Text
}

/// Percent-encode the characters cURL would encode when `-G` moves data into the query
/// string (unreserved characters plus `=`/`&`/`+` stay readable).
fn encode_query_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'=' | b'&' | b'+');
        if keep {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// Generate Postman-style cURL text for a request: each flag on its own
/// `\`-continued line, or one single line when `line_width` is `0`.
///
/// The round trip holds: `parse_curl(&generate_curl(request, _))` keeps method, URL, headers
/// (order and values) and the body text.
pub fn generate_curl(request: &HttpRequest, line_width: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push("curl".to_string());
    parts.push("--location".to_string());
    parts.push(format!("--request {}", method_str(&request.method)));
    parts.push(quote(&request.url));

    for row in &request.headers {
        if !row.enabled || row.key.is_empty() {
            continue;
        }
        parts.push(format!("--header {}", quote(&format!("{}: {}", row.key, row.value))));
    }

    if let AuthConfig::Basic(basic) = &request.auth
        && (!basic.username.is_empty() || !basic.password.is_empty())
    {
        parts.push(format!("--user {}", quote(&format!("{}:{}", basic.username, basic.password))));
    }

    if !request.settings.verify_ssl {
        parts.push("--insecure".to_string());
    }
    match request.settings.http_version {
        HttpVersion::Http1 => parts.push("--http1.1".to_string()),
        HttpVersion::Http2 => parts.push("--http2".to_string()),
        HttpVersion::Auto => {}
    }
    if request.settings.timeout_ms > 0 {
        parts.push(format!("--max-time {}", format_secs(request.settings.timeout_ms)));
    }
    if request.settings.max_redirects != RequestSettings::default().max_redirects {
        parts.push(format!("--max-redirs {}", request.settings.max_redirects));
    }

    match &request.body {
        Body::None => {}
        Body::FormData(rows) => {
            for row in rows {
                if !row.enabled || row.key.is_empty() {
                    continue;
                }
                if row.kind == KeyValueKind::File {
                    parts.push(format!("--form {}", quote(&format!("{}=@{}", row.key, row.value))));
                } else {
                    parts.push(format!("--form {}", quote(&format!("{}={}", row.key, row.value))));
                }
            }
        }
        Body::UrlEncoded(rows) => {
            let text = rows.iter().filter(|r| r.enabled && !r.key.is_empty()).map(|r| format!("{}={}", r.key, r.value)).collect::<Vec<_>>().join("&");
            parts.push(format!("--data-raw {}", quote(&text)));
        }
        Body::Raw { text, .. } => {
            parts.push(format!("--data-raw {}", quote(text)));
        }
        Body::Binary { path, .. } => {
            if !path.is_empty() {
                parts.push(format!("--data-binary {}", quote(&format!("@{path}"))));
            }
        }
        Body::GraphQL { query, variables, operation_name, .. } => {
            let mut object = serde_json::Map::new();
            object.insert("query".to_string(), serde_json::Value::String(query.clone()));
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(variables) {
                object.insert("variables".to_string(), parsed);
            } else if !variables.is_empty() {
                object.insert("variables".to_string(), serde_json::Value::String(variables.clone()));
            }
            if let Some(name) = operation_name {
                object.insert("operationName".to_string(), serde_json::Value::String(name.clone()));
            }
            parts.push(format!("--data-raw {}", quote(&serde_json::Value::Object(object).to_string())));
        }
        _ => {}
    }

    if line_width == 0 {
        parts.join(" ")
    } else {
        // Postman style: `curl --location --request METHOD 'url'` stays on the first line;
        // every later flag gets its own `\`-continued, two-space-indented line.
        let mut out = parts[..4.min(parts.len())].join(" ");
        for part in &parts[4.min(parts.len())..] {
            out.push_str(" \\\n  ");
            out.push_str(part);
        }
        out
    }
}

fn method_str(method: &HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Head => "HEAD",
        HttpMethod::Options => "OPTIONS",
    }
}

/// Single-quote a value for the shell, escaping inner quotes as `'\'`-style chunks.
fn quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn format_secs(ms: u64) -> String {
    let secs = ms / 1000;
    let rest = ms % 1000;
    if rest == 0 { secs.to_string() } else { format!("{secs}.{rest:03}") }
}
