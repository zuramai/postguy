//! The AI Agent loop: one user message in, provider round trips and workspace tool calls until
//! the model replies. Every step is saved to the workspace's agent chat as it happens, so a
//! crash loses at most the step in flight (the next turn answers its calls as interrupted).
//!
//! Safety: the agent sees only [`AGENT_TOOLS`]; destructive tools and export overwrites wait for
//! [`AgentHost::approve`]; every tool call runs inside `catch_unwind` (AGENTS.md 4); results
//! sent back to the provider are capped at [`TOOL_RESULT_CAP`].

use std::panic::AssertUnwindSafe;

use postguy_engine::{AiTool, Cancel, Engine};
use postguy_model::{AgentChat, AiProvider, ChatMessage, ToolCall};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::error::{AutomationError, Result};
use crate::session::{bool_arg, string_arg};
use crate::tools::{Automation, ToolDef, tools};

/// The tools the agent may call. Opening other workspaces, the agent's own tools, imports,
/// cookies, gRPC and WebSocket tools are deliberately absent.
pub const AGENT_TOOLS: &[&str] = &[
    "workspace_info",
    "collection_list",
    "collection_get",
    "collection_create",
    "collection_rename",
    "collection_duplicate",
    "collection_update",
    "collection_delete",
    "folder_create",
    "folder_get",
    "folder_update",
    "folder_rename",
    "folder_delete",
    "request_create",
    "request_get",
    "request_update",
    "request_move",
    "request_duplicate",
    "request_delete",
    "request_send",
    "env_list",
    "env_get",
    "env_create",
    "env_rename",
    "env_duplicate",
    "env_delete",
    "env_set_active",
    "env_set_var",
    "env_unset_var",
    "globals_get",
    "globals_set_var",
    "collection_run",
    "run_list",
    "run_get",
    "run_export",
    "variables_resolve",
];

/// Provider round trips per turn.
pub const MAX_STEPS: usize = 25;
/// Tool calls run per provider reply; the rest are answered with an error.
pub const MAX_CALLS_PER_STEP: usize = 16;
/// The largest tool result sent back to the provider.
pub const TOOL_RESULT_CAP: usize = 64 * 1024;
/// Bytes of serialized history sent per completion; older turns are left out.
pub const HISTORY_BUDGET: usize = 400_000;

const SYSTEM_PROMPT: &str = "You are the AI Agent inside Postguy, a desktop API client. You work on the user's open workspace only through the tools you are given: collections, folders, requests, environments, globals and the collection runner.
Rules:
- Tools take ids, never names. Find ids with collection_list, collection_get, folder_get and env_list. If a name matches more than one item, ask which one.
- To change a request, call request_get, edit the returned request.kind object and send the whole object back as `definition` to request_update. Keep every field you do not mean to change. Headers, params, body, auth, settings and scripts live in kind; docs live in `description` (Markdown). The pre-request script is kind.scripts.prerequest.text, the post-response script is kind.scripts.test.text and the Test tab is kind.tests (assertions and tests.script.text); scripts use pm.test, pm.expect, pm.response, pm.environment and pm.collectionVariables.
- Collection and folder variables, auth, scripts and descriptions change with collection_update and folder_update. Environment variables change with env_set_var and env_unset_var.
- collection_run runs a collection, or one folder with `folder`. Its result has a `link`. If the user asked for a file, call run_export with the run id, the format matching the extension (csv, json, junit, html, xlsx, pdf) and the user's path; relative paths are inside the workspace folder; with no path use exports/<collection name>-<run id>.<ext>. Otherwise reply with the totals and the `link` exactly as given, so the user can open the run.
- Deletes and overwrites need the user's approval and Postguy asks for it. If a result says the user declined, do not retry; ask what to do instead.
- \"Tidy up\" means consistent names, related requests grouped in folders and short descriptions. Never delete requests unless the user asked.
- Never invent ids, URLs or secrets. Keep replies short and say what you changed by name. Plain text; no tables.";

const STOPPED: &str = "Stopped.";

/// What the loop reports while it runs.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// A message was added to the chat (and saved).
    Message(ChatMessage),
    /// What the agent is doing now ("Thinking…", a tool title).
    Busy(String),
    /// A tool changed the workspace; views showing it should reload.
    WorkspaceChanged,
}

/// The frontend side of a turn.
pub trait AgentHost {
    fn event(&mut self, event: AgentEvent);
    /// Ask the user to approve a destructive step; `false` declines it.
    fn approve(&mut self, summary: &str) -> bool;
}

/// One tool call of a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepSummary {
    pub tool: String,
    pub label: String,
    pub ok: bool,
}

/// A collection run the agent started; `url` opens it in the Runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunLink {
    pub collection: String,
    pub run: String,
    pub url: String,
}

/// The result of one turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TurnOutcome {
    pub reply: String,
    pub steps: Vec<StepSummary>,
    pub links: Vec<RunLink>,
    /// Summaries of the destructive steps the user declined.
    pub declined: Vec<String>,
}

/// A turn that could not finish. `recorded` is true when the error is already in the chat as a
/// notice; otherwise the chat was left untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnError {
    pub message: String,
    pub recorded: bool,
}

impl std::fmt::Display for TurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TurnError {}

fn unrecorded(message: impl Into<String>) -> TurnError {
    TurnError { message: message.into(), recorded: false }
}

/// The chat being written during a turn: every push is emitted and saved.
struct Transcript<'a> {
    engine: &'a Engine,
    chat: AgentChat,
    host: &'a mut dyn AgentHost,
}

impl Transcript<'_> {
    fn push(&mut self, message: ChatMessage) -> Result<(), TurnError> {
        self.chat.messages.push(message.clone());
        self.host.event(AgentEvent::Message(message));
        self.engine.save_agent_chat(&self.chat).map_err(|err| unrecorded(format!("the agent chat could not be saved: {err}")))
    }

    fn notice(&mut self, text: &str, error: bool) -> Result<(), TurnError> {
        self.push(ChatMessage::Notice { text: text.to_string(), error, time_ms: postguy_util::time::now_unix_ms() })
    }
}

impl Automation {
    /// Run one agent turn for `text`. Settings, key and chat problems found before anything is
    /// sent leave the chat untouched (`recorded: false`).
    pub fn agent_turn(&self, text: &str, host: &mut dyn AgentHost, cancel: &Cancel) -> Result<TurnOutcome, TurnError> {
        let engine = self.engine().map_err(|err| unrecorded(err.to_string()))?;
        let ai = engine.settings().ai;
        let label = ai.provider.label();
        if ai.model().is_empty() {
            return Err(unrecorded(format!("Choose a model for {label} in Settings → AI Agent and click Save.")));
        }
        let key = match engine.ai_api_key(ai.provider) {
            Ok(Some(key)) => key,
            Ok(None) => return Err(unrecorded(format!("No API key for {label}. Add it in Settings → AI Agent and click Save."))),
            Err(err) => return Err(unrecorded(err.to_string())),
        };
        let mut chat = engine.agent_chat().map_err(|err| unrecorded(err.to_string()))?;
        repair_interrupted(&mut chat.messages);
        let turn_start = chat.messages.len();
        let mut transcript = Transcript { engine: &engine, chat, host };
        transcript.push(ChatMessage::User { text: text.to_string(), time_ms: postguy_util::time::now_unix_ms() })?;

        let ai_tools: Vec<AiTool> = tools()
            .iter()
            .filter(|def| AGENT_TOOLS.contains(&def.name))
            .map(|def| AiTool { name: def.name.to_string(), description: def.description.to_string(), input_schema: def.input_schema.clone() })
            .collect();
        let system = system_prompt(&engine);
        let mut outcome = TurnOutcome::default();

        for _ in 0..MAX_STEPS {
            if cancel.is_cancelled() {
                transcript.notice(STOPPED, false)?;
                return Ok(outcome);
            }
            transcript.host.event(AgentEvent::Busy("Thinking…".to_string()));
            let history = sendable(&transcript.chat.messages, turn_start);
            let reply = match engine.ai_complete(&key, &system, &history, &ai_tools, cancel) {
                Ok(reply) => reply,
                Err(_) if cancel.is_cancelled() => {
                    transcript.notice(STOPPED, false)?;
                    return Ok(outcome);
                }
                Err(err) => {
                    let message = err.to_string();
                    transcript.notice(&message, true)?;
                    return Err(TurnError { message, recorded: true });
                }
            };
            transcript.push(ChatMessage::Assistant {
                text: reply.text.clone(),
                tool_calls: reply.tool_calls.clone(),
                time_ms: postguy_util::time::now_unix_ms(),
            })?;
            if reply.tool_calls.is_empty() {
                outcome.reply = reply.text;
                return Ok(outcome);
            }
            for (index, call) in reply.tool_calls.iter().enumerate() {
                let step = self.agent_step(index, call, transcript.host, cancel, &mut outcome);
                transcript.push(ChatMessage::Tool {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    content: step.content,
                    ok: step.ok,
                    label: step.label.clone(),
                    time_ms: postguy_util::time::now_unix_ms(),
                })?;
                if step.changed {
                    transcript.host.event(AgentEvent::WorkspaceChanged);
                }
                outcome.steps.push(StepSummary { tool: call.name.clone(), label: step.label, ok: step.ok });
            }
        }
        let text = format!("Stopped after {MAX_STEPS} steps. Send “continue” to let the agent go on.");
        transcript.notice(&text, false)?;
        outcome.reply = text;
        Ok(outcome)
    }

    /// Run (or refuse, or ask approval for) one tool call.
    fn agent_step(&self, index: usize, call: &ToolCall, host: &mut dyn AgentHost, cancel: &Cancel, outcome: &mut TurnOutcome) -> Step {
        let def = tools().iter().find(|def| def.name == call.name && AGENT_TOOLS.contains(&def.name));
        let failed = |label: String, error: String| Step { content: cap(json!({"error": error}).to_string()), ok: false, label, changed: false };
        if index >= MAX_CALLS_PER_STEP {
            let error = format!("too many tool calls in one step; at most {MAX_CALLS_PER_STEP} run, ask again for the rest");
            return failed(step_label(def, call, false), error);
        }
        if cancel.is_cancelled() {
            return failed(step_label(def, call, false), "stopped by the user".to_string());
        }
        let Some(def) = def else {
            return failed(
                step_label(None, call, false),
                format!("unknown tool `{}`; the available tools are: {}", call.name, AGENT_TOOLS.join(", ")),
            );
        };
        if def.destructive || (def.name == "run_export" && self.export_overwrites(&call.arguments)) {
            let summary = self.describe_action(def, &call.arguments);
            if !host.approve(&summary) {
                let content = json!({"declined": summary, "note": "The user declined. Do not retry; ask the user what to do instead."});
                outcome.declined.push(summary);
                return Step { content: cap(content.to_string()), ok: false, label: step_label(Some(def), call, false), changed: false };
            }
        }
        host.event(AgentEvent::Busy(step_label(Some(def), call, true)));
        let (value, ok) = match std::panic::catch_unwind(AssertUnwindSafe(|| self.call(def.name, &call.arguments))) {
            Ok(Ok(value)) => (model_view(def.name, value, &mut outcome.links), true),
            Ok(Err(err)) => (json!({"error": err.to_string()}), false),
            Err(_) => (json!({"error": "the tool crashed; nothing else was changed"}), false),
        };
        let content =
            serde_json::to_string(&value).unwrap_or_else(|err| json!({"error": format!("the result could not be encoded: {err}")}).to_string());
        Step { content: cap(content), ok, label: step_label(Some(def), call, ok), changed: ok && !def.read_only }
    }

    /// `run_export` would replace a file that already exists.
    fn export_overwrites(&self, args: &Value) -> bool {
        args.get("out").and_then(Value::as_str).and_then(|out| self.resolve_output(out).ok()).is_some_and(|path| path.exists())
    }

    /// The approval text for a destructive step, with names looked up read-only.
    fn describe_action(&self, def: &ToolDef, args: &Value) -> String {
        let arg = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let lookup = |tool: &str, call_args: Value, pointer: &str, fallback: String| {
            self.call(tool, &call_args).ok().and_then(|value| value.pointer(pointer).and_then(Value::as_str).map(str::to_string)).unwrap_or(fallback)
        };
        let collection = || lookup("collection_get", json!({"collection": arg("collection")}), "/collection/name", arg("collection"));
        let environment = || lookup("env_get", json!({"environment": arg("environment")}), "/environment/name", arg("environment"));
        match def.name {
            "collection_delete" => format!("Delete collection “{}”", collection()),
            "folder_delete" => {
                let folder = lookup("folder_get", json!({"collection": arg("collection"), "folder": arg("folder")}), "/folder/name", arg("folder"));
                format!("Delete folder “{folder}” in “{}”", collection())
            }
            "request_delete" => {
                let request =
                    lookup("request_get", json!({"collection": arg("collection"), "request": arg("request")}), "/request/name", arg("request"));
                format!("Delete request “{request}” in “{}”", collection())
            }
            "env_delete" => format!("Delete environment “{}”", environment()),
            "env_unset_var" => format!("Remove variable “{}” from environment “{}”", arg("key"), environment()),
            "run_export" => format!("Overwrite {}", arg("out")),
            _ => format!("{} ({args})", def.title),
        }
    }

    /// The `agent_chat` tool: one headless turn; destructive steps follow `allow_destructive`.
    pub(crate) fn agent_chat_tool(&self, tool: &str, args: &Map<String, Value>) -> Result<Value> {
        let message = string_arg(args, tool, "message")?;
        if message.trim().is_empty() {
            return Err(AutomationError::BadArgument {
                tool: tool.to_string(),
                argument: "message".to_string(),
                expected: "a non-empty message".to_string(),
                got: "empty text".to_string(),
            });
        }
        let mut host = HeadlessHost { allow_destructive: bool_arg(args, tool, "allow_destructive", false)? };
        let cancel = Cancel::new();
        self.register_cancel("agent", &cancel);
        let result = self.agent_turn(&message, &mut host, &cancel);
        self.unregister_cancel("agent");
        let outcome = result.map_err(|err| AutomationError::Failed { message: err.message })?;
        serde_json::to_value(outcome).map_err(|err| AutomationError::Failed { message: format!("encoding the agent reply: {err}") })
    }
}

struct Step {
    content: String,
    ok: bool,
    label: String,
    changed: bool,
}

/// The `agent_chat` tool's host: no UI, approvals answered by the caller's flag.
struct HeadlessHost {
    allow_destructive: bool,
}

impl AgentHost for HeadlessHost {
    fn event(&mut self, _event: AgentEvent) {}

    fn approve(&mut self, _summary: &str) -> bool {
        self.allow_destructive
    }
}

fn system_prompt(engine: &Engine) -> String {
    let folder = engine.store_paths().workspace().file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let now = postguy_util::time::now_unix_ms();
    let stamp = postguy_util::time::format_datetime(now, postguy_util::time::local_offset_seconds(now), postguy_util::time::DateStyle::Full, now);
    let date: String = stamp.chars().take(10).collect();
    format!("{SYSTEM_PROMPT}\nWorkspace folder: {folder}. Today: {date}.")
}

/// The UI step text: the tool title, the `name` (or `key`) argument, and whether it failed.
fn step_label(def: Option<&ToolDef>, call: &ToolCall, ok: bool) -> String {
    let mut label = def.map_or_else(|| call.name.clone(), |def| def.title.to_string());
    let key = if call.name == "env_set_var" { "key" } else { "name" };
    if let Some(name) = call.arguments.get(key).and_then(Value::as_str) {
        label.push_str(&format!(" · \"{name}\""));
    }
    if !ok {
        label.push_str(" failed");
    }
    label
}

/// Cap a tool result at [`TOOL_RESULT_CAP`] bytes, cut at a char boundary.
fn cap(mut text: String) -> String {
    if text.len() <= TOOL_RESULT_CAP {
        return text;
    }
    let mut cut = TOOL_RESULT_CAP;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let dropped = text.len().saturating_sub(cut);
    text.truncate(cut);
    text.push_str(&format!("…[truncated {dropped} bytes; ask for a narrower read]"));
    text
}

/// What the model sees of a tool result: a run becomes its totals and a Runner link.
fn model_view(tool: &str, value: Value, links: &mut Vec<RunLink>) -> Value {
    if tool != "collection_run" {
        return value;
    }
    let Ok(result) = serde_json::from_value::<postguy_engine::RunResult>(value.clone()) else {
        return value;
    };
    let link = RunLink {
        collection: result.collection_id.0.clone(),
        run: result.id.clone(),
        url: format!("postguy://run/{}/{}", result.collection_id.0, result.id),
    };
    let view = json!({
        "run": result.id,
        "collection": result.collection_id.0,
        "collectionName": result.collection_name,
        "passed": result.passed(),
        "totals": result.totals,
        "stopped": result.stopped,
        "link": link.url,
    });
    links.push(link);
    view
}

/// The non-notice messages sent to the provider, oldest turns left out past [`HISTORY_BUDGET`].
/// The current turn (from `turn_start`) is always kept.
fn sendable(messages: &[ChatMessage], turn_start: usize) -> Vec<ChatMessage> {
    let kept: Vec<(usize, &ChatMessage, usize)> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| !matches!(message, ChatMessage::Notice { .. }))
        .map(|(index, message)| (index, message, serde_json::to_vec(message).map(|bytes| bytes.len()).unwrap_or(0)))
        .collect();
    let mut total: usize = kept.iter().map(|(_, _, size)| *size).fold(0, usize::saturating_add);
    let mut start = 0;
    while total > HISTORY_BUDGET {
        let next = kept
            .iter()
            .enumerate()
            .skip(start + 1)
            .find(|(_, (index, message, _))| *index <= turn_start && matches!(message, ChatMessage::User { .. }))
            .map(|(position, _)| position);
        let Some(next) = next else { break };
        let dropped: usize = kept.get(start..next).unwrap_or_default().iter().map(|(_, _, size)| *size).fold(0, usize::saturating_add);
        total = total.saturating_sub(dropped);
        start = next;
    }
    kept.get(start..).unwrap_or_default().iter().map(|(_, message, _)| (*message).clone()).collect()
}

/// Answer the tool calls a crash or quit left without a result, so providers accept the history.
fn repair_interrupted(messages: &mut Vec<ChatMessage>) {
    let mut index = 0;
    while index < messages.len() {
        let calls: Vec<(String, String)> = match messages.get(index) {
            Some(ChatMessage::Assistant { tool_calls, .. }) => tool_calls.iter().map(|call| (call.id.clone(), call.name.clone())).collect(),
            _ => Vec::new(),
        };
        index += 1;
        if calls.is_empty() {
            continue;
        }
        let mut end = index;
        while matches!(messages.get(end), Some(ChatMessage::Tool { .. })) {
            end += 1;
        }
        let answered: Vec<&str> = messages
            .get(index..end)
            .unwrap_or_default()
            .iter()
            .filter_map(|message| match message {
                ChatMessage::Tool { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
            .collect();
        let missing: Vec<ChatMessage> = calls
            .iter()
            .filter(|(id, _)| !answered.contains(&id.as_str()))
            .map(|(id, name)| ChatMessage::Tool {
                call_id: id.clone(),
                name: name.clone(),
                content: r#"{"error":"not run: the chat was interrupted"}"#.to_string(),
                ok: false,
                label: "Interrupted".to_string(),
                time_ms: 0,
            })
            .collect();
        let added = missing.len();
        messages.splice(end..end, missing);
        index = end + added;
    }
}

/// `agent_settings_get`: provider, endpoint, models and whether a key is stored. Never the key.
pub(crate) fn settings_json(engine: &Engine) -> Result<Value> {
    let ai = engine.settings().ai;
    let models: Map<String, Value> = AiProvider::ALL.iter().map(|provider| (provider.slug().to_string(), json!(ai.models.get(*provider)))).collect();
    Ok(json!({
        "provider": ai.provider.slug(),
        "providers": AiProvider::ALL.iter().map(|provider| provider.slug()).collect::<Vec<_>>(),
        "endpoint": ai.openai_endpoint,
        "models": models,
        "model": ai.model(),
        "apiKeyStored": engine.ai_api_key(ai.provider)?.is_some(),
    }))
}

/// Validate an OpenAI-compatible endpoint: trimmed, no trailing `/`, empty resets to the default.
pub fn normalize_endpoint(given: &str) -> std::result::Result<String, String> {
    let endpoint = given.trim().trim_end_matches('/');
    if endpoint.is_empty() {
        return Ok(postguy_model::agent::DEFAULT_OPENAI_ENDPOINT.to_string());
    }
    let lower = endpoint.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(endpoint.to_string())
    } else {
        Err("the endpoint must start with http:// or https://".to_string())
    }
}

/// `agent_settings_set`.
pub(crate) fn settings_set(engine: &Engine, tool: &str, args: &Map<String, Value>) -> Result<Value> {
    let bad = |argument: &str, expected: &str, got: String| AutomationError::BadArgument {
        tool: tool.to_string(),
        argument: argument.to_string(),
        expected: expected.to_string(),
        got,
    };
    let mut settings = engine.settings();
    if let Some(slug) = crate::session::optional_string(args, tool, "provider")? {
        settings.ai.provider =
            AiProvider::from_slug(slug.trim()).ok_or_else(|| bad("provider", "openai-compatible, claude, gemini or openrouter", slug))?;
    }
    if let Some(endpoint) = crate::session::optional_string(args, tool, "endpoint")? {
        settings.ai.openai_endpoint =
            normalize_endpoint(&endpoint).map_err(|_| bad("endpoint", "a URL starting with http:// or https://", endpoint))?;
    }
    if let Some(model) = crate::session::optional_string(args, tool, "model")? {
        *settings.ai.models.get_mut(settings.ai.provider) = model.trim().to_string();
    }
    let api_key = crate::session::optional_string(args, tool, "api_key")?;
    let remove = bool_arg(args, tool, "remove_api_key", false)?;
    if api_key.is_some() && remove {
        return Err(bad("remove_api_key", "false when api_key is given (store a key or remove it, not both)", "true".to_string()));
    }
    let provider = settings.ai.provider;
    engine.update_settings(settings)?;
    match api_key {
        Some(key) => engine.set_ai_api_key(provider, Some(&key))?,
        None if remove => engine.set_ai_api_key(provider, None)?,
        None => {}
    }
    settings_json(engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(ids: &[&str]) -> ChatMessage {
        ChatMessage::Assistant {
            text: String::new(),
            tool_calls: ids
                .iter()
                .map(|id| ToolCall { id: (*id).into(), name: "collection_list".into(), arguments: json!({}), provider_data: None })
                .collect(),
            time_ms: 0,
        }
    }

    fn tool(id: &str) -> ChatMessage {
        ChatMessage::Tool { call_id: id.into(), name: "collection_list".into(), content: "{}".into(), ok: true, label: String::new(), time_ms: 0 }
    }

    #[test]
    fn interrupted_calls_are_answered_right_after_their_results() {
        let mut messages = vec![
            ChatMessage::User { text: "a".into(), time_ms: 0 },
            assistant(&["1", "2"]),
            tool("1"),
            ChatMessage::User { text: "b".into(), time_ms: 0 },
            assistant(&["3"]),
        ];
        repair_interrupted(&mut messages);
        let ids: Vec<String> = messages
            .iter()
            .map(|message| match message {
                ChatMessage::Tool { call_id, ok, .. } => format!("tool:{call_id}:{ok}"),
                ChatMessage::User { text, .. } => format!("user:{text}"),
                _ => "assistant".to_string(),
            })
            .collect();
        assert_eq!(ids, ["user:a", "assistant", "tool:1:true", "tool:2:false", "user:b", "assistant", "tool:3:false"]);
    }

    #[test]
    fn long_results_are_cut_at_a_char_boundary() {
        let text = "é".repeat(TOOL_RESULT_CAP);
        let capped = cap(text);
        assert!(capped.contains("…[truncated"));
        assert!(capped.len() < TOOL_RESULT_CAP + 80);
    }

    #[test]
    fn old_turns_are_left_out_past_the_budget_but_the_current_turn_stays() {
        let big = "x".repeat(HISTORY_BUDGET / 3);
        let messages = vec![
            ChatMessage::User { text: big.clone(), time_ms: 0 },
            ChatMessage::Notice { text: "local".into(), error: false, time_ms: 0 },
            ChatMessage::User { text: big.clone(), time_ms: 0 },
            ChatMessage::User { text: big, time_ms: 0 },
        ];
        let sent = sendable(&messages, 3);
        assert_eq!(sent.len(), 2, "the oldest turn is dropped, the notice never sent");
        let only_current = sendable(&messages[3..], 0);
        assert_eq!(only_current.len(), 1);
    }
}
