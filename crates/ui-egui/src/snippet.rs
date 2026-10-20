//! Code snippets (PROMPT.md 5.8, M3.9 subset): cURL (the interop generator), JavaScript fetch and
//! Python requests. The remaining languages arrive with M4. The right-panel widget renders the
//! selected language's text and copies it.

use egui::{RichText, Ui};
use postguy_model::{Body, HttpRequest};

use crate::theme::Theme;
use crate::widgets;

/// Languages available in the M3.9 snippet panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnippetLanguage {
    #[default]
    Curl,
    Fetch,
    PythonRequests,
}

impl SnippetLanguage {
    pub fn label(&self) -> &'static str {
        match self {
            SnippetLanguage::Curl => "cURL",
            SnippetLanguage::Fetch => "JavaScript (fetch)",
            SnippetLanguage::PythonRequests => "Python (requests)",
        }
    }

    pub fn all() -> [SnippetLanguage; 3] {
        [SnippetLanguage::Curl, SnippetLanguage::Fetch, SnippetLanguage::PythonRequests]
    }
}

/// Generate the snippet text for a request (variables unresolved, as the panel shows them).
pub fn generate(language: SnippetLanguage, request: &HttpRequest) -> String {
    match language {
        SnippetLanguage::Curl => postguy_interop::generate_curl(request, 100),
        SnippetLanguage::Fetch => fetch(request),
        SnippetLanguage::PythonRequests => python_requests(request),
    }
}

fn body_text(request: &HttpRequest) -> Option<String> {
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
        _ => None,
    }
}

fn headers(request: &HttpRequest) -> Vec<(String, String)> {
    request.headers.iter().filter(|row| row.enabled && !row.key.is_empty()).map(|row| (row.key.clone(), row.value.clone())).collect()
}

fn method(request: &HttpRequest) -> &'static str {
    match request.method {
        postguy_model::HttpMethod::Get => "GET",
        postguy_model::HttpMethod::Post => "POST",
        postguy_model::HttpMethod::Put => "PUT",
        postguy_model::HttpMethod::Patch => "PATCH",
        postguy_model::HttpMethod::Delete => "DELETE",
        postguy_model::HttpMethod::Head => "HEAD",
        postguy_model::HttpMethod::Options => "OPTIONS",
    }
}

fn fetch(request: &HttpRequest) -> String {
    let mut out = String::new();
    out.push_str("const response = await fetch(");
    out.push_str(&js_string(&request.url));
    out.push_str(", {\n");
    out.push_str(&format!("  method: \"{}\",\n", method(request)));
    let headers = headers(request);
    if !headers.is_empty() {
        out.push_str("  headers: {\n");
        for (name, value) in headers {
            out.push_str(&format!("    {}: {},\n", js_string(&name), js_string(&value)));
        }
        out.push_str("  },\n");
    }
    if let Some(body) = body_text(request) {
        out.push_str(&format!("  body: {},\n", js_string(&body)));
    }
    out.push_str("});\n");
    out.push_str("console.log(await response.text());");
    out
}

fn python_requests(request: &HttpRequest) -> String {
    let mut out = String::from("import requests\n\n");
    let headers = headers(request);
    if headers.is_empty() {
        out.push_str(&format!("response = requests.{}(", method(request).to_ascii_lowercase()));
        out.push_str(&py_string(&request.url));
        out.push(')');
    } else {
        out.push_str("headers = {\n");
        for (name, value) in headers {
            out.push_str(&format!("    {}: {},\n", py_string(&name), py_string(&value)));
        }
        out.push_str("}\n\n");
        out.push_str(&format!("response = requests.{}(", method(request).to_ascii_lowercase()));
        out.push_str(&py_string(&request.url));
        out.push_str(", headers=headers");
        if let Some(body) = body_text(request) {
            out.push_str(&format!(", data={}", py_string(&body)));
        }
        out.push(')');
    }
    out.push_str("\nprint(response.text)");
    out
}

fn js_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    format!("\"{escaped}\"")
}

fn py_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    format!("\"{escaped}\"")
}

fn percent(value: &str) -> String {
    value.replace('%', "%25").replace('&', "%26").replace('=', "%3D").replace(' ', "%20")
}

/// The right-panel widget: language menu, copy button and the generated text.
#[derive(Debug, Clone, Default)]
pub struct SnippetPanelState {
    pub language: SnippetLanguage,
}

pub fn snippet_panel(ui: &mut Ui, request: &HttpRequest, state: &mut SnippetPanelState) {
    let theme = Theme::current(ui.ctx());
    ui.horizontal(|ui| {
        ui.heading("Code snippet");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let text = generate(state.language, request);
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(text);
            }
            let mut chosen = Some(state.language.label().to_string());
            let groups = [widgets::SelectGroup {
                label: None,
                items: SnippetLanguage::all().into_iter().map(|language| widgets::SelectItem::new(language.label(), language.label())).collect(),
            }];
            if widgets::select(ui, "snippet-language", &mut chosen, &groups, state.language.label()) {
                state.language =
                    SnippetLanguage::all().into_iter().find(|language| Some(language.label().to_string()) == chosen).unwrap_or(SnippetLanguage::Curl);
            }
        });
    });
    ui.separator();
    let mut text = generate(state.language, request);
    let response = ui.add(egui::TextEdit::multiline(&mut text).code_editor().desired_width(f32::INFINITY).desired_rows(24).interactive(false));
    response.on_hover_text("Variables are shown unresolved here; the Console shows the resolved URL.");
    ui.label(RichText::new(state.language.label()).small().color(theme.palette.text_muted));
}

#[cfg(test)]
mod tests {
    use super::*;
    use postguy_model::{HttpMethod, KeyValue};

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
    fn fetch_and_python_include_method_headers_and_body() {
        let fetch = generate(SnippetLanguage::Fetch, &request());
        assert!(fetch.contains("POST"));
        assert!(fetch.contains("Accept"));
        assert!(fetch.contains("\\\"a\\\":1"));
        let python = generate(SnippetLanguage::PythonRequests, &request());
        assert!(python.contains("requests.post"));
        assert!(python.contains("headers=headers"));
        assert!(python.contains("data="));
    }

    #[test]
    fn curl_uses_the_interop_generator() {
        let curl = generate(SnippetLanguage::Curl, &request());
        assert!(curl.starts_with("curl"));
        assert!(curl.contains("--request POST"));
    }
}
