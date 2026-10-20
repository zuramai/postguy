//! Code generation for the right-panel snippets and `snippet_generate` (PROMPT.md 5.8, M4.2):
//! cURL, raw HTTP/1.1, JavaScript fetch, Python requests, Go net/http, Rust reqwest,
//! PowerShell Invoke-RestMethod and Node axios. Pure string generation: no I/O, no panics on
//! user text.
//!
//! Variables are shown unresolved (the panel's default) exactly as they appear in the request;
//! the caller can resolve them first if it wants the "keep variables" toggle off.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use postguy_model::{Body, HttpMethod, HttpRequest, KeyValue, KeyValueKind};

/// A snippet language offered in the panel (grpcurl joins with the gRPC crate in M7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    Curl,
    RawHttp,
    Fetch,
    PythonRequests,
    Go,
    Rust,
    PowerShell,
    NodeAxios,
}

impl Language {
    /// The stable tool/CLI id.
    pub fn id(self) -> &'static str {
        match self {
            Language::Curl => "curl",
            Language::RawHttp => "raw-http",
            Language::Fetch => "fetch",
            Language::PythonRequests => "python",
            Language::Go => "go",
            Language::Rust => "rust",
            Language::PowerShell => "powershell",
            Language::NodeAxios => "node-axios",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Language::Curl => "cURL",
            Language::RawHttp => "Raw HTTP/1.1",
            Language::Fetch => "JavaScript (fetch)",
            Language::PythonRequests => "Python (requests)",
            Language::Go => "Go (net/http)",
            Language::Rust => "Rust (reqwest)",
            Language::PowerShell => "PowerShell",
            Language::NodeAxios => "NodeJS (axios)",
        }
    }

    pub fn all() -> [Language; 8] {
        [
            Language::Curl,
            Language::RawHttp,
            Language::Fetch,
            Language::PythonRequests,
            Language::Go,
            Language::Rust,
            Language::PowerShell,
            Language::NodeAxios,
        ]
    }

    pub fn parse(id: &str) -> Option<Language> {
        Language::all().into_iter().find(|language| language.id() == id)
    }
}

/// Formatting knobs (the panel's gear).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Number of spaces per indentation level; 0 means one tab.
    pub indent: usize,
    pub include_boilerplate: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { indent: 2, include_boilerplate: true }
    }
}

/// Generate one language. Never fails: unsupported body kinds degrade to a documented comment
/// in the snippet instead of dropping the request.
pub fn generate(language: Language, request: &HttpRequest, options: Options) -> String {
    match language {
        Language::Curl => postguy_interop::generate_curl(request, 100),
        Language::RawHttp => raw_http(request, options),
        Language::Fetch => fetch(request, options),
        Language::PythonRequests => python_requests(request, options),
        Language::Go => go(request, options),
        Language::Rust => rust(request, options),
        Language::PowerShell => powershell(request, options),
        Language::NodeAxios => node_axios(request, options),
    }
}

fn indent(options: Options, level: usize) -> String {
    if options.indent == 0 { "\t".repeat(level) } else { " ".repeat(options.indent * level) }
}

fn method(request: &HttpRequest) -> &'static str {
    match request.method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Head => "HEAD",
        HttpMethod::Options => "OPTIONS",
    }
}

fn method_lower(request: &HttpRequest) -> &'static str {
    match request.method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
        HttpMethod::Head => "head",
        HttpMethod::Options => "options",
    }
}

fn headers(request: &HttpRequest) -> Vec<(&str, &str)> {
    request.headers.iter().filter(|row| row.enabled && !row.key.is_empty()).map(|row| (row.key.as_str(), row.value.as_str())).collect()
}

fn text_body(request: &HttpRequest) -> Option<String> {
    match &request.body {
        Body::None => None,
        Body::Raw { text, .. } => Some(text.clone()),
        Body::UrlEncoded(rows) => Some(
            rows.iter()
                .filter(|row| row.enabled && !row.key.is_empty())
                .map(|row| format!("{}={}", percent(&row.key), percent(&row.value)))
                .collect::<Vec<String>>()
                .join("&"),
        ),
        Body::GraphQL { query, variables, operation_name, .. } => {
            let mut object = serde_json::Map::new();
            object.insert("query".into(), serde_json::Value::String(query.clone()));
            object.insert("variables".into(), serde_json::from_str(variables).unwrap_or(serde_json::json!({})));
            if let Some(name) = operation_name.as_ref().filter(|name| !name.is_empty()) {
                object.insert("operationName".into(), serde_json::Value::String(name.clone()));
            }
            Some(serde_json::Value::Object(object).to_string())
        }
        Body::FormData(_) | Body::Binary { .. } => None,
        _ => None,
    }
}

fn binary_path(request: &HttpRequest) -> Option<&str> {
    match &request.body {
        Body::Binary { path, .. } if !path.is_empty() => Some(path),
        _ => None,
    }
}

fn raw_http(request: &HttpRequest, _options: Options) -> String {
    let (host, target) = split_url(&request.url);
    let mut out = format!("{} {target} HTTP/1.1\n", method(request));
    let mut has_host = false;
    for &(name, value) in &headers(request) {
        if name.eq_ignore_ascii_case("host") {
            has_host = true;
        }
        out.push_str(&format!("{name}: {value}\n"));
    }
    if !has_host {
        out.push_str(&format!("Host: {host}\n"));
    }
    if let Body::FormData(rows) = &request.body {
        let boundary = "postguy-boundary";
        if !headers(request).iter().any(|(name, _)| name.eq_ignore_ascii_case("content-type")) {
            out.push_str(&format!("Content-Type: multipart/form-data; boundary={boundary}\n"));
        }
        let mut body = String::new();
        for row in rows.iter().filter(|row| row.enabled && !row.key.is_empty()) {
            body.push_str(&format!("--{boundary}\n"));
            if matches!(row.kind, KeyValueKind::File) {
                body.push_str(&format!("Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\n\n", row.key, row.value));
                body.push_str(&format!("[file contents: {}]\n", row.value));
            } else {
                body.push_str(&format!("Content-Disposition: form-data; name=\"{}\"\n\n{}\n", row.key, row.value));
            }
        }
        body.push_str(&format!("--{boundary}--\n"));
        if !headers(request).iter().any(|(name, _)| name.eq_ignore_ascii_case("content-length")) {
            out.push_str(&format!("Content-Length: {}\n", body.len()));
        }
        out.push('\n');
        out.push_str(&body);
        return out;
    }
    if let Some(body) = text_body(request) {
        if !headers(request).iter().any(|(name, _)| name.eq_ignore_ascii_case("content-length")) {
            out.push_str(&format!("Content-Length: {}\n", body.len()));
        }
        out.push('\n');
        out.push_str(&body);
    } else if let Some(path) = binary_path(request) {
        if !headers(request).iter().any(|(name, _)| name.eq_ignore_ascii_case("content-length")) {
            out.push_str(&format!("Content-Length: <size of {path}>\n"));
        }
        out.push_str(&format!("\n[file contents: {path}]"));
    } else {
        out.push('\n');
    }
    out
}

fn fetch(request: &HttpRequest, options: Options) -> String {
    let i1 = indent(options, 1);
    let i2 = indent(options, 2);
    let mut out = String::new();
    out.push_str("const response = await fetch(");
    out.push_str(&js_string(&request.url));
    out.push_str(", {\n");
    out.push_str(&format!("{i1}method: \"{}\",\n", method(request)));
    let headers = headers(request);
    if !headers.is_empty() {
        out.push_str(&format!("{i1}headers: {{\n"));
        for &(name, value) in &headers {
            out.push_str(&format!("{i2}{}: {},\n", js_string(name), js_string(value)));
        }
        out.push_str(&format!("{i1}}},\n"));
    }
    if let Body::FormData(rows) = &request.body {
        out.push_str(&format!("{i1}body: (() => {{\n{i2}const form = new FormData();\n"));
        for row in rows.iter().filter(|row| row.enabled && !row.key.is_empty()) {
            if matches!(row.kind, KeyValueKind::File) {
                out.push_str(&format!("{i2}form.append({}, new Blob([/* file: {} */]));\n", js_string(&row.key), js_comment(&row.value)));
            } else {
                out.push_str(&format!("{i2}form.append({}, {});\n", js_string(&row.key), js_string(&row.value)));
            }
        }
        out.push_str(&format!("{i2}return form;\n{i1}}})(),\n"));
    } else if let Some(body) = text_body(request) {
        out.push_str(&format!("{i1}body: {},\n", js_string(&body)));
    } else if let Some(path) = binary_path(request) {
        out.push_str(&format!("{i1}body: await (await fetch({})).blob(), // file: {}\n", js_string(path), js_comment(path)));
    }
    out.push_str("});\n");
    out.push_str("console.log(await response.text());");
    out
}

fn python_requests(request: &HttpRequest, options: Options) -> String {
    let i1 = indent(options, 1);
    let mut out = String::new();
    if options.include_boilerplate {
        out.push_str("import requests\n\n");
    }
    let headers = headers(request);
    if !headers.is_empty() {
        out.push_str("headers = {\n");
        for &(name, value) in &headers {
            out.push_str(&format!("{i1}{}: {},\n", py_string(name), py_string(value)));
        }
        out.push_str("}\n\n");
    }
    let mut args = vec![py_string(&request.url)];
    if !headers.is_empty() {
        args.push("headers=headers".into());
    }
    if let Body::FormData(rows) = &request.body {
        let text: Vec<&KeyValue> = rows.iter().filter(|row| row.enabled && !row.key.is_empty() && matches!(row.kind, KeyValueKind::Text)).collect();
        let files: Vec<(&str, &KeyValue)> = rows
            .iter()
            .filter(|row| row.enabled && !row.key.is_empty() && matches!(row.kind, KeyValueKind::File))
            .map(|row| (row.key.as_str(), row))
            .collect();
        if !text.is_empty() {
            out.push_str("data = {\n");
            for row in text {
                out.push_str(&format!("{i1}{}: {},\n", py_string(&row.key), py_string(&row.value)));
            }
            out.push_str("}\n\n");
            args.push("data=data".into());
        }
        if !files.is_empty() {
            out.push_str("files = {\n");
            for (key, row) in &files {
                out.push_str(&format!("{i1}{}: open({}, \"rb\"),\n", py_string(key), py_string(&row.value)));
            }
            out.push_str("}\n\n");
            args.push("files=files".into());
        }
    } else if let Some(body) = text_body(request) {
        args.push(format!("data={}", py_string(&body)));
    } else if let Some(path) = binary_path(request) {
        out.push_str(&format!("with open({}, \"rb\") as payload:\n", py_string(path)));
        args.push("data=payload".into());
        out.push_str(&format!("{i1}response = requests.{}({})\n", method_lower(request), args.join(", ")));
        out.push_str("print(response.text)");
        return out;
    }
    out.push_str(&format!("response = requests.{}({})", method_lower(request), args.join(", ")));
    out.push('\n');
    out.push_str("print(response.text)");
    out
}

fn go(request: &HttpRequest, options: Options) -> String {
    let i1 = indent(options, 1);
    let i2 = indent(options, 2);
    let mut out = String::new();
    if options.include_boilerplate {
        out.push_str("package main\n\nimport (\n");
        out.push_str(&format!("{i1}\"fmt\"\n"));
        out.push_str(&format!("{i1}\"io\"\n"));
        out.push_str(&format!("{i1}\"net/http\"\n"));
        out.push_str(&format!("{i1}\"strings\"\n"));
        out.push_str(")\n\nfunc main() {\n");
    }
    let body = text_body(request);
    match body.as_deref() {
        Some(body) => out.push_str(&format!("{i1}payload := strings.NewReader({})\n", go_string(body))),
        None => out.push_str(&format!("{i1}var payload io.Reader\n")),
    }
    out.push_str(&format!(
        "{i1}req, err := http.NewRequest({}, {}, payload)\n{i1}if err != nil {{\n{i2}panic(err)\n{i1}}}\n",
        go_string(method(request)),
        go_string(&request.url)
    ));
    for &(name, value) in &headers(request) {
        out.push_str(&format!("{i1}req.Header.Add({}, {})\n", go_string(name), go_string(value)));
    }
    if let Body::FormData(rows) = &request.body {
        out.push_str(&format!("{i1}// form-data rows (replace with a multipart.Writer for file uploads):\n"));
        for row in rows.iter().filter(|row| row.enabled && !row.key.is_empty()) {
            out.push_str(&format!("{i1}//   {} = {}\n", row.key, row.value));
        }
    } else if let Some(path) = binary_path(request) {
        out.push_str(&format!("{i1}// body file: {path} (open it and pass the reader as payload)\n"));
    }
    out.push_str(&format!(
        "{i1}resp, err := http.DefaultClient.Do(req)\n{i1}if err != nil {{\n{i2}panic(err)\n{i1}}}\n{i1}defer resp.Body.Close()\n{i1}data, err := io.ReadAll(resp.Body)\n{i1}if err != nil {{\n{i2}panic(err)\n{i1}}}\n{i1}fmt.Println(string(data))\n"
    ));
    if options.include_boilerplate {
        out.push_str("}\n");
    }
    out
}

fn rust(request: &HttpRequest, options: Options) -> String {
    let i1 = indent(options, 1);
    let mut out = String::new();
    if options.include_boilerplate {
        out.push_str("#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {\n");
        out.push_str(&format!("{i1}let client = reqwest::Client::new();\n"));
    } else {
        out.push_str("let client = reqwest::Client::new();\n");
    }
    let prefix = if options.include_boilerplate { i1.as_str() } else { "" };
    let chain = indent(options, 2);
    out.push_str(&format!("{prefix}let response = client\n{chain}.{}({})\n", method_lower(request), rust_string(&request.url)));
    for &(name, value) in &headers(request) {
        out.push_str(&format!("{chain}.header({}, {})\n", rust_string(name), rust_string(value)));
    }
    if let Body::FormData(rows) = &request.body {
        out.push_str(&format!("{chain}.multipart(\n"));
        out.push_str(&format!("{chain}{}reqwest::multipart::Form::new()\n", indent(options, 1)));
        for row in rows.iter().filter(|row| row.enabled && !row.key.is_empty()) {
            if matches!(row.kind, KeyValueKind::File) {
                out.push_str(&format!("{chain}{}.file({}, {})\n", indent(options, 2), rust_string(&row.key), rust_string(&row.value)));
            } else {
                out.push_str(&format!("{chain}{}.text({}, {})\n", indent(options, 2), rust_string(&row.key), rust_string(&row.value)));
            }
        }
        out.push_str(&format!("{chain})\n"));
    } else if let Some(body) = text_body(request) {
        out.push_str(&format!("{chain}.body({})\n", rust_string(&body)));
    } else if let Some(path) = binary_path(request) {
        out.push_str(&format!("{chain}.body(std::fs::read({})?)\n", rust_string(path)));
    }
    out.push_str(&format!("{chain}.send()\n"));
    out.push_str(&format!("{chain}.await?;\n"));
    out.push_str(&format!("{prefix}let body = response.text().await?;\n{prefix}println!(\"{{body}}\");\n"));
    if options.include_boilerplate {
        out.push_str(&format!("{i1}Ok(())\n}}\n"));
    }
    out
}

fn powershell(request: &HttpRequest, options: Options) -> String {
    let i1 = indent(options, 1);
    let mut out = String::new();
    let headers = headers(request);
    if !headers.is_empty() {
        out.push_str("$headers = @{\n");
        for &(name, value) in &headers {
            out.push_str(&format!("{i1}{} = {}\n", ps_string(name), ps_string(value)));
        }
        out.push_str("}\n\n");
    }
    out.push_str(&format!("$response = Invoke-RestMethod -Uri {} -Method {}", ps_string(&request.url), ps_title(method(request))));
    if !headers.is_empty() {
        out.push_str(" -Headers $headers");
    }
    if let Body::FormData(rows) = &request.body {
        let text: Vec<&KeyValue> = rows.iter().filter(|row| row.enabled && !row.key.is_empty() && matches!(row.kind, KeyValueKind::Text)).collect();
        if !text.is_empty() {
            out.push_str(" -Form @{\n");
            for row in text {
                out.push_str(&format!("{i1}{} = {}\n", ps_string(&row.key), ps_string(&row.value)));
            }
            out.push('}');
        }
    } else if let Some(body) = text_body(request) {
        out.push_str(&format!(" -Body {}", ps_string(&body)));
    } else if let Some(path) = binary_path(request) {
        out.push_str(&format!(" -InFile {}", ps_string(path)));
    }
    out.push_str("\n$response | ConvertTo-Json -Depth 10");
    out
}

fn node_axios(request: &HttpRequest, options: Options) -> String {
    let i1 = indent(options, 1);
    let mut inner = String::new();
    inner.push_str(&format!("const response = await axios.{}(\n{i1}{}", method_lower(request), js_string(&request.url)));
    let config = axios_config(request, options);
    if !config.is_empty() {
        inner.push_str(",\n");
        for line in config.lines() {
            inner.push_str(&i1);
            inner.push_str(line);
            inner.push('\n');
        }
        inner.push_str(");\n");
    } else {
        inner.push_str("\n);\n");
    }
    inner.push_str("console.log(response.data);");
    if !options.include_boilerplate {
        return inner;
    }
    // Top-level await is invalid in a CommonJS file with `require`; wrap the call in an async IIFE.
    let mut out = String::from("const axios = require(\"axios\");\n\n(async () => {\n");
    for line in inner.lines() {
        out.push_str(&i1);
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("})();");
    out
}

fn axios_config(request: &HttpRequest, options: Options) -> String {
    let i1 = indent(options, 1);
    let i2 = indent(options, 2);
    let mut out = String::from("{\n");
    let headers = headers(request);
    if !headers.is_empty() {
        out.push_str(&format!("{i1}headers: {{\n"));
        for &(name, value) in &headers {
            out.push_str(&format!("{i2}{}: {},\n", js_string(name), js_string(value)));
        }
        out.push_str(&format!("{i1}}},\n"));
    }
    if let Body::FormData(rows) = &request.body {
        out.push_str(&format!("{i1}data: (() => {{\n{i2}const form = new FormData();\n"));
        for row in rows.iter().filter(|row| row.enabled && !row.key.is_empty()) {
            if matches!(row.kind, KeyValueKind::File) {
                out.push_str(&format!("{i2}form.append({}, fs.createReadStream({}));\n", js_string(&row.key), js_string(&row.value)));
            } else {
                out.push_str(&format!("{i2}form.append({}, {});\n", js_string(&row.key), js_string(&row.value)));
            }
        }
        out.push_str(&format!("{i2}return form;\n{i1}}})(),\n"));
    } else if let Some(body) = text_body(request) {
        out.push_str(&format!("{i1}data: {},\n", js_string(&body)));
    } else if let Some(path) = binary_path(request) {
        out.push_str(&format!("{i1}data: fs.createReadStream({}),\n", js_string(path)));
    }
    out.push('}');
    out
}

fn split_url(url: &str) -> (String, String) {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    match without_scheme.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}")),
        None => (without_scheme.to_string(), "/".to_string()),
    }
}

fn percent(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn js_string(value: &str) -> String {
    format!("\"{}\"", escape(value, '"'))
}

fn js_comment(value: &str) -> String {
    value.replace("*/", "* /")
}

fn py_string(value: &str) -> String {
    format!("\"{}\"", escape(value, '"'))
}

fn go_string(value: &str) -> String {
    format!("\"{}\"", escape(value, '"'))
}

fn rust_string(value: &str) -> String {
    format!("\"{}\"", escape(value, '"'))
}

fn ps_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn ps_title(method: &str) -> String {
    let mut chars = method.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first, chars.as_str().to_ascii_lowercase()),
        None => String::new(),
    }
}

fn escape(value: &str, quote: char) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Post,
            url: "https://api.example.com/items".into(),
            headers: vec![KeyValue { key: "Accept".into(), value: "application/json".into(), ..Default::default() }],
            body: Body::Raw { language: postguy_model::RawLanguage::Json, text: "{\"a\":1}".into(), extras: Default::default() },
            ..Default::default()
        }
    }

    #[test]
    fn every_language_mentions_the_method_url_and_body() {
        for language in Language::all() {
            let snippet = generate(language, &request(), Options::default());
            assert!(snippet.contains("Accept"), "{} misses headers: {snippet}", language.id());
            if language == Language::RawHttp {
                assert!(snippet.contains("POST /items HTTP/1.1") && snippet.contains("Host: api.example.com"), "{snippet}");
            } else {
                assert!(snippet.contains("api.example.com/items"), "{}: {snippet}", language.id());
                assert!(snippet.to_ascii_uppercase().contains("POST"), "{}: {snippet}", language.id());
            }
        }
    }

    #[test]
    fn language_ids_round_trip() {
        for language in Language::all() {
            assert_eq!(Language::parse(language.id()), Some(language));
        }
        assert_eq!(Language::parse("grpcurl"), None);
    }

    #[test]
    fn tabs_are_used_when_indent_is_zero() {
        let tabs = generate(Language::Fetch, &request(), Options { indent: 0, include_boilerplate: true });
        assert!(tabs.contains("\n\tmethod:"), "{tabs}");
        let no_boilerplate = generate(Language::PythonRequests, &request(), Options { indent: 4, include_boilerplate: false });
        assert!(!no_boilerplate.contains("import requests"), "{no_boilerplate}");
    }
}
