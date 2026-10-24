//! HAR 1.2 import: one request per recorded entry, with the recorded response as an example.
//!
//! A HAR file is a hostile input: the entry count and every body are capped, base64 failures
//! drop the body (with a report entry), and nothing is fetched. Behaviour comes from the public
//! HAR 1.2 specification.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use postguy_model::{
    Body, Collection, Example, ExampleBody, HttpMethod, HttpRequest, Id, Item, KeyValue, KeyValueKind, RawLanguage, Request, RequestKind,
};
use serde_json::Value;

use crate::error::SpecError;
use crate::postman::ImportReport;

/// Most entries imported from one HAR.
pub const MAX_ENTRIES: usize = 10_000;
/// Largest response body kept per entry; larger ones are dropped with a report entry.
pub const MAX_BODY_BYTES: usize = 5 << 20;

/// Whether `v` has the HAR shape (`log.entries` is an array).
pub fn is_har(v: &Value) -> bool {
    v.pointer("/log/entries").is_some_and(Value::is_array)
}

/// Import a HAR document into one flat collection.
pub fn import_har(v: &Value) -> Result<(Collection, ImportReport), SpecError> {
    let Some(entries) = v.pointer("/log/entries").and_then(Value::as_array) else {
        return Err(SpecError::Invalid { what: "the document", detail: "it has no `log.entries` array, so it is not a HAR file".to_string() });
    };
    let mut report = ImportReport { collections: 1, ..ImportReport::default() };
    let name = v
        .pointer("/log/pages/0/title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Imported HAR")
        .to_string();
    let mut items = Vec::new();
    let mut skipped_headers = false;
    for (index, entry) in entries.iter().enumerate() {
        if index >= MAX_ENTRIES {
            report.normal("log.entries", format!("more than {MAX_ENTRIES} entries; the rest were not imported"));
            break;
        }
        let path = format!("log.entries[{index}]");
        if let Some(request) = entry_request(entry, &path, &mut report, &mut skipped_headers) {
            report.requests += 1;
            items.push(Item::Request(Box::new(request)));
        }
    }
    if skipped_headers {
        report.normal("log.entries", "pseudo-headers, Content-Length and Host were not imported");
    }
    Ok((Collection { id: Id::generate(), name, items, ..Collection::default() }, report))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn method_of(raw: &str, path: &str, report: &mut ImportReport) -> HttpMethod {
    match raw.to_ascii_uppercase().as_str() {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        "HEAD" => HttpMethod::Head,
        "OPTIONS" => HttpMethod::Options,
        other => {
            report.normal(path, format!("method {other} is not supported; imported as GET"));
            HttpMethod::Get
        }
    }
}

/// Header rows without pseudo-headers (`:authority`), `Content-Length` and `Host`.
fn header_rows(headers: Option<&Value>, skip_length_and_host: bool, skipped: &mut bool) -> Vec<KeyValue> {
    let mut rows = Vec::new();
    for header in headers.and_then(Value::as_array).into_iter().flatten().take(500) {
        let name = text(header, "name");
        if name.is_empty() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if name.starts_with(':') || (skip_length_and_host && (lower == "content-length" || lower == "host")) {
            *skipped = true;
            continue;
        }
        rows.push(KeyValue { key: name.to_string(), value: text(header, "value").to_string(), ..KeyValue::default() });
    }
    rows
}

fn entry_request(entry: &Value, path: &str, report: &mut ImportReport, skipped_headers: &mut bool) -> Option<Request> {
    let request = entry.get("request")?;
    let url = text(request, "url");
    if url.is_empty() {
        report.drop(path, "the entry has no request URL");
        return None;
    }
    let method = method_of(text(request, "method"), path, report);
    let mut http = HttpRequest { method, url: url.to_string(), ..HttpRequest::default() };
    http.headers = header_rows(request.get("headers"), true, skipped_headers);
    if let Some(post) = request.get("postData") {
        http.body = body_of(post, path, report);
    }
    let parsed = postguy_urlkit::Url::parse(url);
    let mut shown = if parsed.path.is_empty() { "/".to_string() } else { parsed.path.clone() };
    if shown.chars().count() > 200 {
        shown = shown.chars().take(200).collect();
    }
    let name = format!("{} {shown}", postguy_http_method_text(method));
    let examples: Vec<Example> = response_example(entry.get("response"), path, report, skipped_headers).into_iter().collect();
    report.examples += examples.len();
    Some(Request { id: Id::generate(), name, kind: RequestKind::Http(http), examples, ..Request::default() })
}

fn postguy_http_method_text(method: HttpMethod) -> &'static str {
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

fn body_of(post: &Value, path: &str, report: &mut ImportReport) -> Body {
    let mime = text(post, "mimeType").to_ascii_lowercase();
    let body_text = text(post, "text");
    let params = post.get("params").and_then(Value::as_array).filter(|params| !params.is_empty());
    if mime.contains("multipart/form-data")
        && let Some(params) = params
    {
        let mut rows = Vec::new();
        for param in params.iter().take(500) {
            let name = text(param, "name");
            let file = param.get("fileName").and_then(Value::as_str).is_some();
            if file {
                report.normal(path, format!("file part {name} needs a file chosen"));
            }
            rows.push(KeyValue {
                key: name.to_string(),
                value: if file { String::new() } else { text(param, "value").to_string() },
                kind: if file { KeyValueKind::File } else { KeyValueKind::Text },
                content_type: param.get("contentType").and_then(Value::as_str).map(str::to_string),
                ..KeyValue::default()
            });
        }
        return Body::FormData(rows);
    }
    if mime.contains("application/x-www-form-urlencoded") {
        let rows: Vec<KeyValue> = match params {
            Some(params) => params
                .iter()
                .take(500)
                .map(|param| KeyValue { key: text(param, "name").to_string(), value: text(param, "value").to_string(), ..KeyValue::default() })
                .collect(),
            None => postguy_urlkit::params_of_query(body_text)
                .into_iter()
                .map(|param| KeyValue { key: param.key, value: param.value, ..KeyValue::default() })
                .collect(),
        };
        if !rows.is_empty() {
            return Body::UrlEncoded(rows);
        }
    }
    if body_text.is_empty() {
        return Body::None;
    }
    let language = if mime.contains("json") {
        RawLanguage::Json
    } else if mime.contains("xml") {
        RawLanguage::Xml
    } else if mime.contains("html") {
        RawLanguage::Html
    } else if mime.contains("javascript") {
        RawLanguage::JavaScript
    } else {
        RawLanguage::Text
    };
    Body::Raw { language, text: body_text.to_string(), extras: Default::default() }
}

fn response_example(response: Option<&Value>, path: &str, report: &mut ImportReport, skipped_headers: &mut bool) -> Option<Example> {
    let response = response?;
    let status = response.get("status").and_then(Value::as_u64).and_then(|status| u16::try_from(status).ok()).filter(|status| *status > 0)?;
    let status_text = text(response, "statusText").trim().to_string();
    let name = if status_text.is_empty() { status.to_string() } else { format!("{status} {status_text}") };
    let content = response.get("content");
    let raw = content.map(|content| text(content, "text")).unwrap_or_default();
    let base64 = content.is_some_and(|content| text(content, "encoding").eq_ignore_ascii_case("base64"));
    let bytes = if raw.is_empty() {
        None
    } else if base64 {
        let cleaned: String = raw.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        match STANDARD.decode(cleaned.trim_end_matches('=')).or_else(|_| STANDARD.decode(&cleaned)) {
            Ok(bytes) => Some(bytes),
            Err(_) => {
                report.drop(path, "the response body is not valid base64 and was dropped");
                None
            }
        }
    } else {
        Some(raw.as_bytes().to_vec())
    };
    let bytes = bytes.filter(|bytes| {
        let fits = bytes.len() <= MAX_BODY_BYTES;
        if !fits {
            report.drop(path, format!("the response body is over {MAX_BODY_BYTES} bytes and was dropped"));
        }
        fits
    });
    let language = content
        .map(|content| text(content, "mimeType").to_ascii_lowercase())
        .map(|mime| if mime.contains("json") { RawLanguage::Json } else { RawLanguage::Text });
    Some(Example {
        id: Id::generate(),
        name,
        status: Some(status),
        status_text: Some(status_text).filter(|text| !text.is_empty()),
        headers: header_rows(response.get("headers"), false, skipped_headers),
        body: bytes.map(|bytes| ExampleBody { bytes, ..ExampleBody::default() }),
        language,
        ..Example::default()
    })
}
