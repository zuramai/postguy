//! Request building (PROMPT.md 5.3.2 step 6): body bytes for every mode, the automatic header
//! set in a documented order, and the rule that a user header always wins over an automatic
//! one with the same name (case-insensitive). No I/O except the file callback for `binary` and
//! `form-data` file rows, which the caller bounds by the working-directory rule.
//!
//! Wire ORDER of headers: the user's enabled rows in their table order first, then the
//! automatic headers not overridden, in the order listed in [`automatic_headers`]. HTTP/1
//! names go out lowercased by hyper's codec (http::HeaderName has no case); the Console shows
//! the order that was requested here. The absence of a request-token header is deliberate
//! (no Postman-Token equivalent).

use postguy_model::{Body, KeyValue, RawLanguage};
use serde_json::Value;

use crate::error::{HttpError, Result};

/// How to load a file row: the caller enforces the working-directory rule and size caps.
pub type LoadFile<'a> = &'a dyn Fn(&str) -> std::result::Result<Vec<u8>, String>;

/// Pre-resolved request pieces (the pipeline resolves variables before calling in).
pub struct BuildInput<'a> {
    pub method: &'a str,
    /// Fully resolved URL (path vars substituted, query applied, percent-encoded).
    pub url: &'a str,
    /// User header rows, resolved and in table order (disabled rows are filtered by the caller
    /// or skipped here; both are safe).
    pub headers: &'a [KeyValue],
    pub body: &'a Body,
    /// File loader for `binary` / form-data file rows; `None` refuses file bodies with a
    /// readable error instead of guessing a path.
    pub load_file: Option<LoadFile<'a>>,
    /// The "Send no-cache header" setting (PROMPT.md 5.3.2, default on).
    pub send_no_cache: bool,
}

/// The request ready for [`crate::Client::send`]: ordered headers and the exact body bytes.
#[derive(Debug, Clone)]
pub struct BuiltRequest {
    /// The validated method text (uppercase), e.g. "GET".
    pub method: String,
    pub url: String,
    /// Ordered (name as the user typed / as generated, value) pairs. Names go to the wire
    /// lowercased by HTTP/1; this list is the Console's and the review view's truth.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Build-time notes for the Console (a JSON validity hint never blocks a send).
    pub warnings: Vec<String>,
}

/// Map the model method to its wire text.
pub fn method_text(method: postguy_model::HttpMethod) -> &'static str {
    match method {
        postguy_model::HttpMethod::Get => "GET",
        postguy_model::HttpMethod::Post => "POST",
        postguy_model::HttpMethod::Put => "PUT",
        postguy_model::HttpMethod::Patch => "PATCH",
        postguy_model::HttpMethod::Delete => "DELETE",
        postguy_model::HttpMethod::Head => "HEAD",
        postguy_model::HttpMethod::Options => "OPTIONS",
    }
}

/// Build the wire request.
pub fn build(input: BuildInput<'_>) -> Result<BuiltRequest> {
    let mut warnings = Vec::new();
    let (body, body_content_type) = build_body(input.body, input.load_file, &mut warnings)?;

    // User headers first, in order; an invalid name/value is the caller's error.
    let mut headers: Vec<(String, String)> = Vec::new();
    for row in input.headers.iter().filter(|row| row.enabled) {
        if row.key.trim().is_empty() {
            continue;
        }
        let name = row.key.trim().to_string();
        validate_header(&name, &row.value, input.url)?;
        headers.push((name, row.value.clone()));
    }

    // Automatic headers not present in the user set (case-insensitive).
    let host = host_of(input.url)?;
    let mut automatic = automatic_headers(&host, input.send_no_cache);
    if let Some(content_type) = body_content_type {
        automatic.push(("Content-Type".to_string(), content_type));
    }
    if !body.is_empty() {
        automatic.push(("Content-Length".to_string(), body.len().to_string()));
    }
    for (name, value) in automatic {
        let overridden = headers.iter().any(|(candidate, _)| candidate.eq_ignore_ascii_case(&name));
        if !overridden {
            headers.push((name, value));
        }
    }

    Ok(BuiltRequest { method: normalize_method(input.method)?, url: input.url.to_string(), headers, body, warnings })
}

impl BuiltRequest {
    /// Convert to the client's send spec (timeout and the response cap travel separately:
    /// they are request SETTINGS, not wire data).
    pub fn into_spec(self, timeout_ms: u64, max_body_bytes: usize) -> crate::RequestSpec {
        let method = hyper::Method::from_bytes(self.method.as_bytes()).unwrap_or(hyper::Method::GET);
        crate::RequestSpec { method, url: self.url, headers: self.headers, body: self.body, timeout_ms, max_body_bytes, decode_compression: true }
    }
}

/// The automatic header set and its order (PROMPT.md 5.3.2; Cookie arrives with the jar in
/// M2.6; auth-generated headers are appended by the auth layer before this builder runs).
pub fn automatic_headers(host: &str, send_no_cache: bool) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Host".to_string(), host.to_string()),
        ("User-Agent".to_string(), format!("Postguy/{}", env!("CARGO_PKG_VERSION"))),
        ("Accept".to_string(), "*/*".to_string()),
        ("Accept-Encoding".to_string(), "gzip, deflate, br".to_string()),
        ("Connection".to_string(), "keep-alive".to_string()),
    ];
    if send_no_cache {
        headers.push(("Cache-Control".to_string(), "no-cache".to_string()));
    }
    headers
}

fn host_of(url: &str) -> Result<String> {
    let parsed = postguy_urlkit::Url::parse(url);
    if parsed.authority.is_empty() {
        return Err(HttpError::BadUrl { url: url.to_string(), reason: "missing host".to_string() });
    }
    Ok(parsed.authority)
}

fn normalize_method(text: &str) -> Result<String> {
    let method = text.trim().to_ascii_uppercase();
    let known = matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS");
    if known {
        Ok(method)
    } else {
        Err(HttpError::BadUrl {
            url: String::new(),
            reason: format!("unknown method `{text}`; allowed: GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS"),
        })
    }
}

fn validate_header(name: &str, value: &str, url: &str) -> Result<()> {
    if name.bytes().any(|byte| byte <= b' ' || byte >= 0x7F || byte == b':') {
        return Err(HttpError::BadUrl { url: url.to_string(), reason: format!("invalid header name `{name}`") });
    }
    if value.contains('\r') || value.contains('\n') {
        return Err(HttpError::BadUrl { url: url.to_string(), reason: format!("header `{name}` contains a newline") });
    }
    Ok(())
}

/// Body bytes and the Content-Type the mode derives (None = leave it to the user / no body).
fn build_body(body: &Body, load_file: Option<LoadFile<'_>>, warnings: &mut Vec<String>) -> Result<(Vec<u8>, Option<String>)> {
    match body {
        Body::None => Ok((Vec::new(), None)),
        Body::Raw { language, text, .. } => Ok((text.as_bytes().to_vec(), Some(language.content_type().to_string()))),
        Body::UrlEncoded(rows) => {
            let mut encoded = String::new();
            for row in rows.iter().filter(|row| row.enabled) {
                if row.key.is_empty() && row.value.is_empty() {
                    continue; // the kept-but-ignored empty row (PROMPT.md 5.3.2)
                }
                if !encoded.is_empty() {
                    encoded.push('&');
                }
                encoded.push_str(&postguy_urlkit::encode_query(&row.key));
                encoded.push('=');
                encoded.push_str(&postguy_urlkit::encode_query(&row.value));
            }
            Ok((encoded.into_bytes(), Some("application/x-www-form-urlencoded".to_string())))
        }
        Body::GraphQL { query, operation_name, variables, .. } => {
            let variables = parse_graphql_variables(variables, warnings);
            let mut payload = serde_json::Map::new();
            payload.insert("query".to_string(), Value::String(query.clone()));
            payload.insert("variables".to_string(), variables);
            if let Some(name) = operation_name.as_ref().filter(|name| !name.is_empty()) {
                payload.insert("operationName".to_string(), Value::String(name.clone()));
            }
            let bytes = serde_json::to_vec(&Value::Object(payload)).map_err(|err| HttpError::Config { reason: err.to_string() })?;
            Ok((bytes, Some("application/json".to_string())))
        }
        Body::Binary { path, .. } => {
            let Some(load) = load_file else {
                return Err(HttpError::Config { reason: format!("binary body file `{path}` needs a file loader (working-directory rule)") });
            };
            match load(path) {
                Ok(bytes) => Ok((bytes, None)),
                Err(err) => Err(HttpError::Config { reason: format!("binary body `{path}`: {err}") }),
            }
        }
        Body::FormData(rows) => {
            let boundary = boundary();
            let mut out: Vec<u8> = Vec::new();
            for row in rows.iter().filter(|row| row.enabled) {
                let name = &row.key;
                if name.is_empty() && row.value.is_empty() {
                    continue;
                }
                out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                match row.kind {
                    postguy_model::KeyValueKind::Text => {
                        out.extend_from_slice(format!("Content-Disposition: form-data; name=\"{}\"\r\n", escape_field(name)).as_bytes());
                        if let Some(content_type) = &row.content_type {
                            out.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
                        }
                        out.extend_from_slice(b"\r\n");
                        out.extend_from_slice(row.value.as_bytes());
                        out.extend_from_slice(b"\r\n");
                    }
                    postguy_model::KeyValueKind::File => {
                        let Some(load) = load_file else {
                            return Err(HttpError::Config { reason: format!("form-data file row `{name}` needs a file loader") });
                        };
                        let bytes = load(&row.value).map_err(|err| HttpError::Config { reason: format!("form-data file `{}`: {err}", row.value) })?;
                        let filename = std::path::Path::new(&row.value)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "file".to_string());
                        out.extend_from_slice(
                            format!("Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n", escape_field(name), escape_field(&filename))
                                .as_bytes(),
                        );
                        if let Some(content_type) = &row.content_type {
                            out.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
                        } else {
                            out.extend_from_slice(b"Content-Type: application/octet-stream\r\n");
                        }
                        out.extend_from_slice(b"\r\n");
                        out.extend_from_slice(&bytes);
                        out.extend_from_slice(b"\r\n");
                    }
                }
            }
            out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
            Ok((out, Some(format!("multipart/form-data; boundary={boundary}"))))
        }
        // `Body` is #[non_exhaustive]: a future variant without a builder is a readable error,
        // never a panic (neither-crash rule).
        _ => Err(HttpError::Config { reason: "unsupported body variant; update the request builder".to_string() }),
    }
}

/// GraphQL `variables`: a JSON object ships as-is; empty or invalid text becomes `{}` with a
/// console warning (the editor only hints, it never blocks a send — PROMPT.md 5.3.2).
fn parse_graphql_variables(text: &str, warnings: &mut Vec<String>) -> Value {
    if text.trim().is_empty() {
        return Value::Object(serde_json::Map::new());
    }
    match serde_json::from_str::<Value>(text) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) => {
            warnings.push("GraphQL variables must be a JSON object; sent as {}".to_string());
            Value::Object(serde_json::Map::new())
        }
        Err(err) => {
            warnings.push(format!("GraphQL variables are not valid JSON ({err}); sent as {{}}"));
            Value::Object(serde_json::Map::new())
        }
    }
}

/// A boundary that cannot collide with typical payload bytes: UUID text with a prefix.
fn boundary() -> String {
    format!("postguy-boundary-{}", uuid_like())
}

fn uuid_like() -> String {
    // No uuid dependency in this crate: the boundary needs uniqueness, not a UUID standard.
    format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..))
}

/// Quote/backslash escaping for multipart field names and filenames (RFC 7578 requires
/// percent/quote handling; a quote or CR/LF must not break the part header).
fn escape_field(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace(['\r', '\n'], "")
}

/// The Content-Type a raw body language sets (kept here so the builder never consults the
/// model's settings tab).
pub fn raw_language_content_type(language: RawLanguage) -> &'static str {
    language.content_type()
}

#[cfg(test)]
mod tests {
    use super::*;
    use postguy_model::{KeyValueKind, RawLanguage};

    fn rows(pairs: &[(&str, &str, bool)]) -> Vec<KeyValue> {
        pairs
            .iter()
            .map(|(key, value, enabled)| KeyValue { key: key.to_string(), value: value.to_string(), enabled: *enabled, ..Default::default() })
            .collect()
    }

    #[test]
    fn automatic_headers_are_in_the_documented_order_and_absent_overridden_ones() {
        let user = rows(&[("accept", "application/json", true), ("X-Probe", "alpha", true)]);
        let built = build(BuildInput {
            method: "GET",
            url: "http://localhost:8080/echo",
            headers: &user,
            body: &Body::None,
            load_file: None,
            send_no_cache: true,
        })
        .expect("build");
        let names: Vec<&str> = built.headers.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["accept", "X-Probe", "Host", "User-Agent", "Accept-Encoding", "Connection", "Cache-Control"]);
        assert_eq!(built.headers[1].1, "alpha");
        assert!(built.headers.iter().any(|(name, value)| name == "User-Agent" && value.starts_with("Postguy/")));
        // The user's `accept` wins; the automatic Accept is gone.
        assert!(!built.headers.iter().any(|(name, value)| name == "Accept" && value == "*/*"));
    }

    #[test]
    fn user_content_type_wins_and_body_length_is_calculated() {
        let user = rows(&[("Content-Type", "application/custom", true)]);
        let body = Body::Raw { language: RawLanguage::Json, text: "{\"a\":1}".to_string(), extras: Default::default() };
        let built = build(BuildInput { method: "POST", url: "http://h/x", headers: &user, body: &body, load_file: None, send_no_cache: false })
            .expect("build");
        let content_types: Vec<&str> =
            built.headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case("content-type")).map(|(_, value)| value.as_str()).collect();
        assert_eq!(content_types, vec!["application/custom"], "one content type, the user's");
        assert!(built.headers.iter().any(|(name, value)| name == "Content-Length" && value == "7"));
        assert!(!built.headers.iter().any(|(name, _)| name == "Cache-Control"), "no-cache off");
    }

    #[test]
    fn urlencoded_rows_encode_and_skip_disabled_and_empty_rows() {
        let body = Body::UrlEncoded(rows(&[("a b", "c&d", true), ("empty", "", true), ("off", "x", false), ("", "", true)]));
        let built =
            build(BuildInput { method: "POST", url: "http://h/x", headers: &[], body: &body, load_file: None, send_no_cache: true }).expect("build");
        assert_eq!(built.body, b"a%20b=c%26d&empty=".to_vec());
        assert_eq!(
            built.headers.iter().find(|(name, _)| name == "Content-Type").map(|(_, value)| value.as_str()),
            Some("application/x-www-form-urlencoded")
        );
    }

    #[test]
    fn multipart_has_boundary_text_and_file_parts() {
        let body = Body::FormData(vec![
            KeyValue { key: "field".into(), value: "value".into(), enabled: true, ..Default::default() },
            KeyValue {
                key: "upload".into(),
                value: "files/report.pdf".into(),
                enabled: true,
                kind: KeyValueKind::File,
                content_type: Some("application/pdf".into()),
                ..Default::default()
            },
        ]);
        let load = |path: &str| -> std::result::Result<Vec<u8>, String> {
            if path == "files/report.pdf" { Ok(b"%PDF-fake".to_vec()) } else { Err("missing".to_string()) }
        };
        let built = build(BuildInput { method: "POST", url: "http://h/x", headers: &[], body: &body, load_file: Some(&load), send_no_cache: true })
            .expect("build");
        let content_type = built.headers.iter().find(|(name, _)| name == "Content-Type").map(|(_, value)| value.as_str()).expect("multipart c-t");
        assert!(content_type.starts_with("multipart/form-data; boundary=postguy-boundary-"), "{content_type}");
        let text = String::from_utf8_lossy(&built.body);
        assert!(text.contains("Content-Disposition: form-data; name=\"field\"\r\n\r\nvalue\r\n"));
        assert!(text.contains("Content-Disposition: form-data; name=\"upload\"; filename=\"report.pdf\""));
        assert!(text.contains("Content-Type: application/pdf"));
        assert!(text.contains("%PDF-fake"));
        assert!(text.trim_end().ends_with("--"), "closing boundary present");
    }

    #[test]
    fn graphql_bodies_serialise_query_variables_and_operation() {
        let body =
            Body::GraphQL { query: "{ me }".into(), operation_name: Some("Me".into()), variables: "{\"id\":1}".into(), extras: Default::default() };
        let warnings: Vec<String> = Vec::new();
        let built = build(BuildInput { method: "POST", url: "http://h/graphql", headers: &[], body: &body, load_file: None, send_no_cache: true })
            .expect("build");
        let parsed: Value = serde_json::from_slice(&built.body).expect("json");
        assert_eq!(parsed["query"], "{ me }");
        assert_eq!(parsed["operationName"], "Me");
        assert_eq!(parsed["variables"]["id"], 1);
        assert_eq!(built.headers.iter().find(|(n, _)| n == "Content-Type").map(|(_, v)| v.as_str()), Some("application/json"));
        // Invalid variables warn but still send with {}.
        let bad = Body::GraphQL { query: "{}".into(), operation_name: None, variables: "{oops".into(), extras: Default::default() };
        let built =
            build(BuildInput { method: "POST", url: "http://h/g", headers: &[], body: &bad, load_file: None, send_no_cache: true }).expect("build");
        let parsed: Value = serde_json::from_slice(&built.body).expect("json");
        assert_eq!(parsed["variables"], serde_json::json!({}));
        assert_eq!(warnings.len(), 0, "warnings live on the built request");
        assert!(!built.warnings.is_empty(), "the invalid-variables hint is reported");
    }

    #[test]
    fn missing_file_loaders_and_bad_headers_are_actionable_errors() {
        let body = Body::Binary { path: "files/a.bin".into(), extras: Default::default() };
        let err = build(BuildInput { method: "POST", url: "http://h/x", headers: &[], body: &body, load_file: None, send_no_cache: true })
            .expect_err("no loader");
        assert!(err.to_string().contains("needs a file loader"), "{err}");

        let row = KeyValue { key: "Bad Header".into(), value: "x".into(), enabled: true, ..Default::default() };
        let err = build(BuildInput { method: "GET", url: "http://h/x", headers: &[row], body: &Body::None, load_file: None, send_no_cache: true })
            .expect_err("bad header name");
        assert!(err.to_string().contains("invalid header name"), "{err}");
        let newline = KeyValue { key: "X".into(), value: "a\r\nInjected: 1".into(), enabled: true, ..Default::default() };
        let err =
            build(BuildInput { method: "GET", url: "http://h/x", headers: &[newline], body: &Body::None, load_file: None, send_no_cache: true })
                .expect_err("header injection refused");
        assert!(err.to_string().contains("newline"), "{err}");
    }

    #[test]
    fn field_escaping_keeps_multipart_headers_intact() {
        assert_eq!(escape_field("a\"b\\c\r\nd"), "a\\\"b\\\\cd");
    }

    #[test]
    fn method_text_covers_every_variant() {
        use postguy_model::HttpMethod::*;
        for method in [Get, Post, Put, Patch, Delete, Head, Options] {
            assert_eq!(normalize_method(method_text(method)).expect("known"), method_text(method));
        }
        assert!(normalize_method("TRACE").is_err(), "unknown methods are refused");
    }

    #[test]
    fn raw_languages_map_to_their_content_types() {
        assert_eq!(raw_language_content_type(RawLanguage::Json), "application/json");
        assert_eq!(raw_language_content_type(RawLanguage::Xml), "application/xml");
    }
}
