//! The provider wire formats: build one non-streaming completion request and parse its reply.
//! Pure functions; `ai_complete` does the I/O.

use postguy_model::{AiProvider, AiSettings, ChatMessage, ToolCall};
use serde_json::{Map, Value, json};

use super::{AiTool, AiTurn};
use crate::error::{EngineError, Result};

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const CLAUDE_URL: &str = "https://api.anthropic.com/v1/messages";
const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
/// Claude requires an explicit output cap.
const CLAUDE_MAX_TOKENS: u32 = 8192;
/// How much of an unreadable error body is quoted back.
const BODY_SNIPPET_CHARS: usize = 300;

/// One provider request, ready for the HTTP client.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WireRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

fn ai_err(message: impl Into<String>) -> EngineError {
    EngineError::Ai { message: message.into() }
}

/// Build the request for the provider chosen in `ai`. `Notice` messages are never sent.
pub(crate) fn build(ai: &AiSettings, key: &str, system: &str, messages: &[ChatMessage], tools: &[AiTool]) -> Result<WireRequest> {
    let model = ai.model();
    if model.is_empty() {
        return Err(ai_err("choose a model in Settings → AI Agent"));
    }
    match ai.provider {
        AiProvider::OpenAiCompatible => {
            let endpoint = ai.openai_endpoint.trim().trim_end_matches('/');
            let lower = endpoint.to_ascii_lowercase();
            if !(lower.starts_with("http://") || lower.starts_with("https://")) {
                return Err(ai_err("the endpoint must start with http:// or https://"));
            }
            let headers = vec![("Authorization".to_string(), format!("Bearer {key}"))];
            Ok(WireRequest { url: format!("{endpoint}/chat/completions"), headers, body: openai_body(model, system, messages, tools) })
        }
        AiProvider::OpenRouter => {
            let headers = vec![("Authorization".to_string(), format!("Bearer {key}")), ("X-Title".to_string(), "Postguy".to_string())];
            Ok(WireRequest { url: OPENROUTER_URL.to_string(), headers, body: openai_body(model, system, messages, tools) })
        }
        AiProvider::Claude => {
            let headers = vec![("x-api-key".to_string(), key.to_string()), ("anthropic-version".to_string(), "2023-06-01".to_string())];
            Ok(WireRequest { url: CLAUDE_URL.to_string(), headers, body: claude_body(model, system, messages, tools) })
        }
        AiProvider::Gemini => {
            let model = gemini_model(model)?;
            let headers = vec![("x-goog-api-key".to_string(), key.to_string())];
            Ok(WireRequest { url: format!("{GEMINI_BASE}/{model}:generateContent"), headers, body: gemini_body(system, messages, tools) })
        }
    }
}

/// Parse a provider reply; non-2xx statuses become an actionable error.
pub(crate) fn parse(provider: AiProvider, status: u16, body: &[u8]) -> Result<AiTurn> {
    if !(200..300).contains(&status) {
        return Err(status_error(provider, status, body));
    }
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| ai_err(format!("{} returned a reply Postguy could not read: {}", provider.label(), snippet(body))))?;
    match provider {
        AiProvider::OpenAiCompatible | AiProvider::OpenRouter => parse_openai(provider, &value),
        AiProvider::Claude => Ok(parse_claude(&value)),
        AiProvider::Gemini => parse_gemini(&value),
    }
}

fn snippet(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let text: String = text.trim().chars().take(BODY_SNIPPET_CHARS).collect();
    if text.is_empty() { "no details".to_string() } else { text }
}

fn status_error(provider: AiProvider, status: u16, body: &[u8]) -> EngineError {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let detail = parsed.as_ref().and_then(|value| {
        let value = match value {
            Value::Array(items) => items.first()?,
            other => other,
        };
        match value.get("error")? {
            Value::String(text) => Some(text.clone()),
            error => error.get("message").and_then(Value::as_str).map(str::to_string),
        }
    });
    let mut message = format!("{} returned HTTP {status}: {}", provider.label(), detail.unwrap_or_else(|| snippet(body)));
    if status == 401 || status == 403 {
        if !message.ends_with('.') {
            message.push('.');
        }
        message.push_str(" Check the API key in Settings → AI Agent.");
    }
    ai_err(message)
}

fn object(pairs: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect::<Map<String, Value>>())
}

// OpenAI chat completions (also OpenRouter).

fn openai_body(model: &str, system: &str, messages: &[ChatMessage], tools: &[AiTool]) -> Value {
    let mut wire = vec![json!({"role": "system", "content": system})];
    for message in messages {
        match message {
            ChatMessage::User { text, .. } => wire.push(json!({"role": "user", "content": text})),
            ChatMessage::Assistant { text, tool_calls, .. } => {
                let content = if text.is_empty() && !tool_calls.is_empty() { Value::Null } else { Value::String(text.clone()) };
                let mut entry = vec![("role", json!("assistant")), ("content", content)];
                if !tool_calls.is_empty() {
                    let calls = tool_calls
                        .iter()
                        .map(|call| json!({"id": call.id, "type": "function", "function": {"name": call.name, "arguments": call.arguments.to_string()}}))
                        .collect();
                    entry.push(("tool_calls", Value::Array(calls)));
                }
                wire.push(object(entry));
            }
            ChatMessage::Tool { call_id, content, .. } => wire.push(json!({"role": "tool", "tool_call_id": call_id, "content": content})),
            ChatMessage::Notice { .. } => {}
        }
    }
    let mut body = vec![("model", json!(model)), ("messages", Value::Array(wire))];
    if !tools.is_empty() {
        let tools = tools
            .iter()
            .map(
                |tool| json!({"type": "function", "function": {"name": tool.name, "description": tool.description, "parameters": tool.input_schema}}),
            )
            .collect();
        body.push(("tools", Value::Array(tools)));
    }
    object(body)
}

fn parse_openai(provider: AiProvider, value: &Value) -> Result<AiTurn> {
    let message = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| ai_err(format!("{} returned no choices", provider.label())))?;
    let text = message.get("content").and_then(Value::as_str).unwrap_or_default().to_string();
    let calls = message.get("tool_calls").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let tool_calls = calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let id = call.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()).map_or_else(|| format!("call_{index}"), str::to_string);
            let function = call.get("function");
            let name = function.and_then(|f| f.get("name")).and_then(Value::as_str).unwrap_or_default().to_string();
            let arguments = match function.and_then(|f| f.get("arguments")) {
                Some(Value::String(raw)) if raw.trim().is_empty() => json!({}),
                Some(Value::String(raw)) => serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.clone())),
                Some(other) => other.clone(),
                None => json!({}),
            };
            ToolCall { id, name, arguments, provider_data: None }
        })
        .collect();
    Ok(AiTurn { text, tool_calls })
}

// Anthropic messages.

fn claude_body(model: &str, system: &str, messages: &[ChatMessage], tools: &[AiTool]) -> Value {
    let mut wire: Vec<Value> = Vec::new();
    let mut results: Vec<Value> = Vec::new();
    for message in messages {
        if let ChatMessage::Tool { call_id, content, ok, .. } = message {
            results.push(json!({"type": "tool_result", "tool_use_id": call_id, "content": content, "is_error": !ok}));
            continue;
        }
        if !results.is_empty() {
            wire.push(json!({"role": "user", "content": std::mem::take(&mut results)}));
        }
        match message {
            ChatMessage::User { text, .. } => wire.push(json!({"role": "user", "content": text})),
            ChatMessage::Assistant { text, tool_calls, .. } => {
                let mut blocks = Vec::new();
                if !text.is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
                blocks.extend(tool_calls.iter().map(|call| json!({"type": "tool_use", "id": call.id, "name": call.name, "input": call.arguments})));
                // An empty assistant turn is refused by the API; skipping it merges the neighbours.
                if !blocks.is_empty() {
                    wire.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            ChatMessage::Tool { .. } | ChatMessage::Notice { .. } => {}
        }
    }
    if !results.is_empty() {
        wire.push(json!({"role": "user", "content": results}));
    }
    let mut body =
        vec![("model", json!(model)), ("max_tokens", json!(CLAUDE_MAX_TOKENS)), ("system", json!(system)), ("messages", Value::Array(wire))];
    if !tools.is_empty() {
        let tools = tools.iter().map(|tool| json!({"name": tool.name, "description": tool.description, "input_schema": tool.input_schema})).collect();
        body.push(("tools", Value::Array(tools)));
    }
    object(body)
}

fn parse_claude(value: &Value) -> AiTurn {
    let blocks = value.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut texts = Vec::new();
    let mut tool_calls = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => texts.push(block.get("text").and_then(Value::as_str).unwrap_or_default().to_string()),
            Some("tool_use") => tool_calls.push(ToolCall {
                id: block.get("id").and_then(Value::as_str).map_or_else(|| format!("toolu_{index}"), str::to_string),
                name: block.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                arguments: block.get("input").cloned().unwrap_or_else(|| json!({})),
                provider_data: None,
            }),
            _ => {}
        }
    }
    AiTurn { text: texts.join("\n\n"), tool_calls }
}

// Gemini generateContent.

fn gemini_model(model: &str) -> Result<&str> {
    let model = model.strip_prefix("models/").unwrap_or(model);
    if model.is_empty() || !model.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')) {
        return Err(ai_err("Gemini model names contain only letters, digits, '.', '-' and '_'"));
    }
    Ok(model)
}

fn gemini_response(content: &str) -> Value {
    match serde_json::from_str::<Value>(content) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(other) => json!({"result": other}),
        Err(_) => json!({"result": content}),
    }
}

fn gemini_body(system: &str, messages: &[ChatMessage], tools: &[AiTool]) -> Value {
    let mut contents: Vec<Value> = Vec::new();
    let mut responses: Vec<Value> = Vec::new();
    for message in messages {
        if let ChatMessage::Tool { name, content, .. } = message {
            responses.push(json!({"functionResponse": {"name": name, "response": gemini_response(content)}}));
            continue;
        }
        if !responses.is_empty() {
            contents.push(json!({"role": "user", "parts": std::mem::take(&mut responses)}));
        }
        match message {
            ChatMessage::User { text, .. } => contents.push(json!({"role": "user", "parts": [{"text": text}]})),
            ChatMessage::Assistant { text, tool_calls, .. } => {
                let mut parts = Vec::new();
                if !text.is_empty() {
                    parts.push(json!({"text": text}));
                }
                for call in tool_calls {
                    let mut part = vec![("functionCall", json!({"name": call.name, "args": call.arguments}))];
                    if let Some(signature) = call.provider_data.as_ref().and_then(|data| data.get("thoughtSignature")) {
                        part.push(("thoughtSignature", signature.clone()));
                    }
                    parts.push(object(part));
                }
                if !parts.is_empty() {
                    contents.push(json!({"role": "model", "parts": parts}));
                }
            }
            ChatMessage::Tool { .. } | ChatMessage::Notice { .. } => {}
        }
    }
    if !responses.is_empty() {
        contents.push(json!({"role": "user", "parts": responses}));
    }
    let mut body = vec![("systemInstruction", json!({"parts": [{"text": system}]})), ("contents", Value::Array(contents))];
    if !tools.is_empty() {
        let declarations: Vec<Value> =
            tools.iter().map(|tool| json!({"name": tool.name, "description": tool.description, "parametersJsonSchema": tool.input_schema})).collect();
        body.push(("tools", json!([{"functionDeclarations": declarations}])));
    }
    object(body)
}

fn parse_gemini(value: &Value) -> Result<AiTurn> {
    let candidate = value.get("candidates").and_then(|c| c.get(0));
    let parts = candidate.and_then(|c| c.get("content")).and_then(|c| c.get("parts")).and_then(Value::as_array).filter(|parts| !parts.is_empty());
    let Some(parts) = parts else {
        let reason = candidate
            .and_then(|c| c.get("finishReason"))
            .or_else(|| value.get("promptFeedback").and_then(|f| f.get("blockReason")))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Err(ai_err(format!("Gemini returned no content (finishReason: {reason})")));
    };
    let mut texts = Vec::new();
    let mut tool_calls = Vec::new();
    for part in parts {
        if let Some(call) = part.get("functionCall") {
            let index = tool_calls.len();
            tool_calls.push(ToolCall {
                id: call.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()).map_or_else(|| format!("gemini-{index}"), str::to_string),
                name: call.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                arguments: call.get("args").cloned().unwrap_or_else(|| json!({})),
                provider_data: part.get("thoughtSignature").map(|signature| json!({"thoughtSignature": signature})),
            });
        } else if part.get("thought").and_then(Value::as_bool) != Some(true)
            && let Some(text) = part.get("text").and_then(Value::as_str)
        {
            texts.push(text.to_string());
        }
    }
    Ok(AiTurn { text: texts.join(""), tool_calls })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(provider: AiProvider, model: &str) -> AiSettings {
        let mut ai = AiSettings { provider, ..AiSettings::default() };
        *ai.models.get_mut(provider) = model.to_string();
        ai
    }

    fn tool() -> AiTool {
        AiTool { name: "collection_list".into(), description: "List collections".into(), input_schema: json!({"type": "object", "properties": {}}) }
    }

    fn call(id: &str, args: Value, provider_data: Option<Value>) -> ToolCall {
        ToolCall { id: id.into(), name: "collection_get".into(), arguments: args, provider_data }
    }

    fn tool_result(id: &str, content: &str, ok: bool) -> ChatMessage {
        ChatMessage::Tool { call_id: id.into(), name: "collection_get".into(), content: content.into(), ok, label: String::new(), time_ms: 0 }
    }

    #[test]
    fn openai_request_stringifies_arguments_and_parses_tool_calls() {
        let ai = AiSettings { openai_endpoint: "http://127.0.0.1:9/v1/".into(), ..settings(AiProvider::OpenAiCompatible, " m ") };
        let messages = vec![
            ChatMessage::User { text: "hi".into(), time_ms: 0 },
            ChatMessage::Notice { text: "local".into(), error: false, time_ms: 0 },
            ChatMessage::Assistant { text: String::new(), tool_calls: vec![call("c1", json!({"collection": "a"}), None)], time_ms: 0 },
            tool_result("c1", "{}", true),
        ];
        let request = build(&ai, "k", "sys", &messages, &[tool()]).unwrap();
        assert_eq!(request.url, "http://127.0.0.1:9/v1/chat/completions");
        assert_eq!(request.headers, vec![("Authorization".to_string(), "Bearer k".to_string())]);
        assert_eq!(request.body["model"], "m");
        let wire = request.body["messages"].as_array().unwrap();
        assert_eq!(wire.len(), 4, "system, user, assistant, tool; the notice stays local");
        assert_eq!(wire[2]["content"], Value::Null);
        assert_eq!(wire[2]["tool_calls"][0]["function"]["arguments"], r#"{"collection":"a"}"#);
        assert_eq!(wire[3]["tool_call_id"], "c1");
        assert_eq!(request.body["tools"][0]["function"]["parameters"]["type"], "object");

        let reply = br#"{"choices":[{"message":{"content":null,"tool_calls":[
            {"id":"x","type":"function","function":{"name":"collection_get","arguments":"{\"collection\":\"b\"}"}},
            {"type":"function","function":{"name":"collection_list","arguments":"not json"}}]}}]}"#;
        let turn = parse(AiProvider::OpenAiCompatible, 200, reply).unwrap();
        assert_eq!(turn.text, "");
        assert_eq!(turn.tool_calls[0].arguments, json!({"collection": "b"}));
        assert_eq!(turn.tool_calls[1].id, "call_1");
        assert_eq!(turn.tool_calls[1].arguments, json!("not json"), "unparseable arguments reach the tool, which refuses them");

        let bad = AiSettings { openai_endpoint: "ftp://x".into(), ..settings(AiProvider::OpenAiCompatible, "m") };
        assert!(build(&bad, "k", "", &[], &[]).unwrap_err().to_string().contains("http:// or https://"));
        assert!(build(&settings(AiProvider::OpenAiCompatible, "  "), "k", "", &[], &[]).unwrap_err().to_string().contains("choose a model"));
    }

    #[test]
    fn claude_merges_consecutive_tool_results_into_one_user_message() {
        let messages = vec![
            ChatMessage::User { text: "hi".into(), time_ms: 0 },
            ChatMessage::Assistant { text: "Looking".into(), tool_calls: vec![call("a", json!({}), None), call("b", json!({}), None)], time_ms: 0 },
            tool_result("a", "one", true),
            tool_result("b", "two", false),
            ChatMessage::User { text: "next".into(), time_ms: 0 },
        ];
        let request = build(&settings(AiProvider::Claude, "c"), "k", "sys", &messages, &[tool()]).unwrap();
        assert_eq!(request.url, CLAUDE_URL);
        assert!(request.headers.contains(&("x-api-key".to_string(), "k".to_string())));
        assert_eq!(request.body["system"], "sys");
        let wire = request.body["messages"].as_array().unwrap();
        assert_eq!(wire.len(), 4);
        assert_eq!(wire[1]["content"][1]["type"], "tool_use");
        let results = wire[2]["content"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[1]["tool_use_id"], "b");
        assert_eq!(results[1]["is_error"], true);
        assert_eq!(wire[3]["content"], "next");
        assert_eq!(request.body["tools"][0]["input_schema"]["type"], "object");

        let reply = br#"{"content":[{"type":"text","text":"ok"},{"type":"tool_use","id":"t1","name":"collection_list","input":{"x":1}}]}"#;
        let turn = parse(AiProvider::Claude, 200, reply).unwrap();
        assert_eq!(turn.text, "ok");
        assert_eq!(turn.tool_calls[0].arguments, json!({"x": 1}));
    }

    #[test]
    fn gemini_returns_the_thought_signature_on_the_same_function_call_part() {
        let reply = br#"{"candidates":[{"content":{"role":"model","parts":[
            {"text":"private","thought":true},
            {"functionCall":{"name":"collection_get","args":{"collection":"a"}},"thoughtSignature":"sig-A"}]}}]}"#;
        let turn = parse(AiProvider::Gemini, 200, reply).unwrap();
        assert_eq!(turn.text, "", "thoughts are not shown");
        assert_eq!(turn.tool_calls[0].id, "gemini-0");
        let messages = vec![
            ChatMessage::User { text: "hi".into(), time_ms: 0 },
            ChatMessage::Assistant { text: turn.text, tool_calls: turn.tool_calls, time_ms: 0 },
            tool_result("gemini-0", r#"{"collection":{"name":"A"}}"#, true),
        ];
        let request = build(&settings(AiProvider::Gemini, "gemini-2.5-flash"), "k", "sys", &messages, &[tool()]).unwrap();
        let contents = request.body["contents"].as_array().unwrap();
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["thoughtSignature"], "sig-A");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["response"]["collection"]["name"], "A");
        assert!(request.body["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"].is_object());

        let empty = br#"{"candidates":[{"finishReason":"SAFETY"}]}"#;
        assert!(parse(AiProvider::Gemini, 200, empty).unwrap_err().to_string().contains("finishReason: SAFETY"));
    }

    #[test]
    fn gemini_model_prefix_is_stripped_and_bad_names_refused() {
        let request = build(&settings(AiProvider::Gemini, "models/gemini-2.5-flash"), "k", "", &[], &[]).unwrap();
        assert_eq!(request.url, format!("{GEMINI_BASE}/gemini-2.5-flash:generateContent"));
        assert!(request.body.get("tools").is_none());
        for bad in ["../x", "a/b", "a:b", "models/", "a b"] {
            assert!(build(&settings(AiProvider::Gemini, bad), "k", "", &[], &[]).is_err(), "{bad}");
        }
    }

    #[test]
    fn http_401_names_the_settings_page() {
        let error = parse(AiProvider::OpenRouter, 401, br#"{"error":{"message":"No auth credentials found"}}"#).unwrap_err().to_string();
        assert!(error.contains("OpenRouter returned HTTP 401: No auth credentials found."), "{error}");
        assert!(error.contains("Settings → AI Agent"), "{error}");
        let error = parse(AiProvider::Gemini, 400, br#"[{"error":{"message":"bad model"}}]"#).unwrap_err().to_string();
        assert!(error.contains("bad model") && !error.contains("API key"), "{error}");
        let long = "é".repeat(400);
        let error = parse(AiProvider::Claude, 500, long.as_bytes()).unwrap_err().to_string();
        assert!(error.ends_with(&"é".repeat(300)), "the snippet is cut at a char boundary");
    }
}
