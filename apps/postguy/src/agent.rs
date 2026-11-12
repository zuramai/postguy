//! The AI Agent pane's glue: runs agent turns on a worker thread over the app's own engine
//! (a second `Engine::open` would be refused by the workspace lock), turns their events into
//! pane rows, routes approvals back to the waiting turn, and saves the AI Agent settings page
//! (the API key goes to the OS keychain on a worker thread, never the UI thread).

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use postguy_automation::{AgentEvent, AgentHost, Automation, Root, TurnError, TurnOutcome};
use postguy_engine::{Cancel, Engine};
use postguy_model::{AgentChat, AiProvider, ChatMessage, Collection, Id, Item};
use postguy_ui_egui::settings::SettingsSection;
use postguy_ui_egui::tabs::TabDoc;
use postguy_ui_egui::{AgentAction, AgentRow, AiKeyStatus};

use crate::App;

/// Walk at most this many items when looking for request names for the examples.
const EXAMPLE_WALK_CAP: usize = 10_000;

/// What the worker threads report to the UI thread.
pub(crate) enum AgentMsg {
    Loaded(Result<AgentChat, String>),
    Event(AgentEvent),
    Approval { summary: String, reply: mpsc::Sender<bool> },
    Done(Result<TurnOutcome, TurnError>),
    Cleared(Result<(), String>),
    KeyStatus(AiProvider, Result<bool, String>),
    KeySaved(Result<String, String>),
    KeyRemoved(Result<(), String>),
}

/// The app side of the AI Agent pane.
#[derive(Default)]
pub(crate) struct AgentUi {
    automation: Option<Arc<Automation>>,
    /// The UI context, so worker threads started outside a frame can still wake it.
    ctx: egui::Context,
    /// The persisted chat as last reported (the pane's rows are built from it).
    messages: Vec<ChatMessage>,
    loaded: bool,
    loading: bool,
    channel: Option<(mpsc::Sender<AgentMsg>, mpsc::Receiver<AgentMsg>)>,
    /// The running turn's cancellation handle.
    job: Option<Cancel>,
    /// The approval the running turn waits for: (row id, answer channel).
    pending: Option<(u64, mpsc::Sender<bool>)>,
    next_approval: u64,
    /// Rows that are not chat messages (approvals, UI-only notices), each shown after the
    /// first `n` messages it was created behind.
    extras: Vec<(usize, AgentRow)>,
    /// The last sent text, put back in the box when a turn fails before it starts.
    last_input: String,
    /// The sidebar revision the examples were built from.
    examples_revision: Option<u64>,
}

impl AgentUi {
    fn sender(&mut self) -> mpsc::Sender<AgentMsg> {
        self.channel.get_or_insert_with(mpsc::channel).0.clone()
    }

    fn notice(&mut self, text: String, error: bool) {
        self.extras.push((self.messages.len(), AgentRow::Notice { text, error }));
    }
}

/// The running turn's host: events and approval requests go to the UI thread.
struct ChannelHost {
    sender: mpsc::Sender<AgentMsg>,
    ctx: egui::Context,
    cancel: Cancel,
}

impl AgentHost for ChannelHost {
    fn event(&mut self, event: AgentEvent) {
        let _ = self.sender.send(AgentMsg::Event(event));
        self.ctx.request_repaint();
    }

    fn approve(&mut self, summary: &str) -> bool {
        let (reply, answer) = mpsc::channel();
        if self.sender.send(AgentMsg::Approval { summary: summary.to_string(), reply }).is_err() {
            return false;
        }
        self.ctx.request_repaint();
        loop {
            match answer.recv_timeout(Duration::from_millis(100)) {
                Ok(approved) => return approved,
                Err(mpsc::RecvTimeoutError::Timeout) if !self.cancel.is_cancelled() => {}
                Err(_) => return false,
            }
        }
    }
}

/// Run `work` on a named worker thread; false when the OS refused to start it.
fn spawn(name: &str, work: impl FnOnce() + Send + 'static) -> bool {
    std::thread::Builder::new().name(name.to_string()).spawn(work).is_ok()
}

fn row_for(message: &ChatMessage) -> Option<AgentRow> {
    match message {
        ChatMessage::User { text, .. } => Some(AgentRow::User(text.clone())),
        ChatMessage::Assistant { text, .. } if !text.trim().is_empty() => Some(AgentRow::Assistant(text.clone())),
        ChatMessage::Assistant { .. } => None,
        ChatMessage::Tool { label, name, ok, .. } => {
            Some(AgentRow::Step { label: if label.is_empty() { name.clone() } else { label.clone() }, ok: *ok })
        }
        ChatMessage::Notice { text, error, .. } => Some(AgentRow::Notice { text: text.clone(), error: *error }),
    }
}

/// The pane rows: the messages, with each extra row after the messages it was created behind.
fn rows_for(messages: &[ChatMessage], extras: &[(usize, AgentRow)]) -> Vec<AgentRow> {
    let mut rows = Vec::with_capacity(messages.len().saturating_add(extras.len()));
    for index in 0..=messages.len() {
        let last = index == messages.len();
        rows.extend(extras.iter().filter(|(anchor, _)| *anchor == index || (last && *anchor > index)).map(|(_, row)| row.clone()));
        if let Some(row) = messages.get(index).and_then(row_for) {
            rows.push(row);
        }
    }
    rows
}

/// `Payments API` → `payments-api`.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "collection".to_string() } else { out }
}

fn request_names(collection: &Collection) -> Vec<&str> {
    let mut names = Vec::new();
    let mut stack: Vec<&Item> = collection.items.iter().collect();
    let mut visited = 0usize;
    while let Some(item) = stack.pop() {
        visited += 1;
        if visited > EXAMPLE_WALK_CAP {
            break;
        }
        match item {
            Item::Request(request) => names.push(request.name.as_str()),
            Item::Folder(folder) => stack.extend(folder.items.iter()),
        }
    }
    names
}

fn pick<T>(items: &[T]) -> Option<&T> {
    if items.is_empty() { None } else { items.get(fastrand::usize(..items.len())) }
}

/// Example prompts built from the user's collections.
fn examples(collections: &[Collection]) -> Vec<String> {
    let environment = "Create an environment \"Local\" with baseUrl = http://localhost:3000".to_string();
    let Some(x) = pick(collections) else {
        return vec!["Create a collection \"My API\" with a GET request to https://example.com".to_string(), environment];
    };
    let mut out = vec![
        format!("Run collection \"{}\" and export the result to {}-results.csv", x.name, slug(&x.name)),
        format!("Tidy up collection \"{}\"", x.name),
    ];
    let with_requests: Vec<(&Collection, Vec<&str>)> =
        collections.iter().map(|collection| (collection, request_names(collection))).filter(|(_, names)| !names.is_empty()).collect();
    if let Some((y, names)) = pick(&with_requests)
        && let Some(request) = pick(names)
    {
        out.push(format!("Fix request \"{request}\" in collection \"{}\"", y.name));
        out.push(format!("Add a test that checks for status 200 to every request in \"{}\"", x.name));
    }
    out.push(environment);
    out
}

impl App {
    fn rebuild_agent_rows(&mut self) {
        self.shell.agent.rows = rows_for(&self.agent.messages, &self.agent.extras);
    }

    /// Per frame: header, examples, the lazy chat load and the worker messages.
    pub(crate) fn poll_agent(&mut self, ctx: &egui::Context) {
        self.agent.ctx = ctx.clone();
        let ai = &self.shell.settings_value.ai;
        let label = ai.provider.label();
        let model = ai.model();
        self.shell.agent.header = if model.is_empty() { format!("{label} · no model set") } else { format!("{label} · {model}") };
        self.shell.agent.configured = !model.is_empty();

        if self.shell.state.agent_open {
            if !self.agent.loaded && !self.agent.loading && self.agent.job.is_none() {
                self.start_agent_load(ctx);
            }
            if self.agent.examples_revision != Some(self.revision) {
                self.shell.agent.examples = examples(&self.shell.sidebar_data.collections);
                self.agent.examples_revision = Some(self.revision);
            }
        }

        let messages: Vec<AgentMsg> = match &self.agent.channel {
            Some((_, receiver)) => receiver.try_iter().collect(),
            None => return,
        };
        let mut rebuild = false;
        for message in messages {
            rebuild |= self.handle_agent_msg(message);
        }
        if rebuild {
            self.rebuild_agent_rows();
        }
    }

    fn start_agent_load(&mut self, ctx: &egui::Context) {
        let Some(engine) = self.engine.clone() else { return };
        let sender = self.agent.sender();
        let ctx = ctx.clone();
        self.agent.loading = true;
        let started = spawn("postguy-agent-load", move || {
            let _ = sender.send(AgentMsg::Loaded(engine.agent_chat().map_err(|err| err.to_string())));
            ctx.request_repaint();
        });
        if !started {
            self.agent.loading = false;
            self.agent.loaded = true;
            self.agent.notice("The agent chat could not be loaded: no thread could be started".into(), true);
            self.rebuild_agent_rows();
        }
    }

    /// Apply one worker message; true when the rows need a rebuild.
    fn handle_agent_msg(&mut self, message: AgentMsg) -> bool {
        match message {
            AgentMsg::Loaded(result) => {
                self.agent.loaded = true;
                self.agent.loading = false;
                match result {
                    Ok(chat) => self.agent.messages = chat.messages,
                    Err(err) => self.agent.notice(format!("The agent chat could not be read: {err}"), true),
                }
                true
            }
            AgentMsg::Event(AgentEvent::Message(message)) => {
                self.agent.messages.push(message);
                true
            }
            AgentMsg::Event(AgentEvent::Busy(label)) => {
                if self.agent.job.is_some() {
                    self.shell.agent.busy = Some(label);
                }
                false
            }
            AgentMsg::Event(AgentEvent::WorkspaceChanged) => {
                self.refresh();
                self.reload_after_agent_edit();
                self.runner.invalidate_runs();
                false
            }
            AgentMsg::Approval { summary, reply } => {
                let id = self.agent.next_approval;
                self.agent.next_approval = self.agent.next_approval.wrapping_add(1);
                self.agent.pending = Some((id, reply));
                self.agent.extras.push((self.agent.messages.len(), AgentRow::Approval { id, summary, decision: None }));
                self.shell.agent.busy = Some("Waiting for your approval".into());
                true
            }
            AgentMsg::Done(result) => {
                self.agent.job = None;
                self.shell.agent.busy = None;
                if let Some((id, reply)) = self.agent.pending.take() {
                    let _ = reply.send(false);
                    self.decide_approval(id, false);
                }
                if let Err(err) = result
                    && !err.recorded
                {
                    self.agent.notice(err.message, true);
                    if self.shell.agent.input.trim().is_empty() {
                        self.shell.agent.input = std::mem::take(&mut self.agent.last_input);
                    }
                }
                true
            }
            AgentMsg::Cleared(result) => {
                match result {
                    Ok(()) => {
                        self.agent.messages.clear();
                        self.agent.extras.clear();
                        self.agent.examples_revision = None;
                    }
                    Err(err) => self.agent.notice(format!("The chat could not be cleared: {err}"), true),
                }
                true
            }
            AgentMsg::KeyStatus(provider, result) => {
                let draft = &mut self.shell.settings.ai;
                if provider == draft.provider {
                    draft.key_status = match result {
                        Ok(true) => AiKeyStatus::Stored,
                        Ok(false) => AiKeyStatus::Missing,
                        Err(err) => AiKeyStatus::Failed(err),
                    };
                }
                false
            }
            AgentMsg::KeySaved(result) => {
                let draft = &mut self.shell.settings.ai;
                draft.saving = false;
                match result {
                    Ok(message) => {
                        draft.api_key.clear();
                        draft.key_status = AiKeyStatus::Stored;
                        draft.message = Some((message, false));
                    }
                    Err(err) => draft.message = Some((err, true)),
                }
                false
            }
            AgentMsg::KeyRemoved(result) => {
                let draft = &mut self.shell.settings.ai;
                draft.saving = false;
                match result {
                    Ok(()) => {
                        draft.key_status = AiKeyStatus::Missing;
                        draft.message = Some(("API key removed".to_string(), false));
                    }
                    Err(err) => draft.message = Some((err, true)),
                }
                false
            }
        }
    }

    fn decide_approval(&mut self, id: u64, approved: bool) {
        for (_, row) in &mut self.agent.extras {
            if let AgentRow::Approval { id: row_id, decision, .. } = row
                && *row_id == id
            {
                *decision = Some(approved);
            }
        }
    }

    /// The automation over the app's engine, confined to the workspace folder.
    fn agent_automation(&mut self, engine: &Engine) -> Result<Arc<Automation>, String> {
        if let Some(automation) = &self.agent.automation {
            return Ok(automation.clone());
        }
        let root = Root::new(engine.store_paths().workspace()).map_err(|err| err.to_string())?;
        let automation = Arc::new(Automation::with_engine(engine.clone(), Some(root)));
        self.agent.automation = Some(automation.clone());
        Ok(automation)
    }

    /// Apply what the pane asked for.
    pub(crate) fn apply_agent(&mut self, action: AgentAction, ctx: &egui::Context) {
        match action {
            AgentAction::Send(text) => self.send_agent(text, ctx),
            AgentAction::Stop => {
                if let Some(cancel) = &self.agent.job {
                    cancel.cancel();
                    self.shell.agent.busy = Some("Stopping…".into());
                }
                if let Some((id, reply)) = self.agent.pending.take() {
                    let _ = reply.send(false);
                    self.decide_approval(id, false);
                    self.rebuild_agent_rows();
                }
            }
            AgentAction::Approve(id) | AgentAction::Decline(id) => {
                let approved = matches!(action, AgentAction::Approve(_));
                if self.agent.pending.as_ref().is_some_and(|(pending, _)| *pending == id)
                    && let Some((_, reply)) = self.agent.pending.take()
                {
                    let _ = reply.send(approved);
                    self.decide_approval(id, approved);
                    self.shell.agent.busy = Some(if approved { "Working…" } else { "Thinking…" }.into());
                    self.rebuild_agent_rows();
                }
            }
            AgentAction::NewChat => {
                if self.agent.job.is_some() {
                    return;
                }
                let Some(engine) = self.engine.clone() else { return };
                let sender = self.agent.sender();
                let ctx = ctx.clone();
                let started = spawn("postguy-agent-clear", move || {
                    let _ = sender.send(AgentMsg::Cleared(engine.save_agent_chat(&AgentChat::default()).map_err(|err| err.to_string())));
                    ctx.request_repaint();
                });
                if !started {
                    self.shell.state.status = "The chat could not be cleared: no thread could be started".into();
                }
            }
            AgentAction::OpenRun { collection, run } => self.open_stored_run(&Id(collection), &run),
            AgentAction::OpenSettings => {
                self.shell.settings.section = SettingsSection::AiAgent;
                self.shell.tabs.open(TabDoc::Settings, "Settings");
            }
        }
    }

    fn send_agent(&mut self, text: String, ctx: &egui::Context) {
        if self.agent.job.is_some() || text.trim().is_empty() {
            return;
        }
        let Some(engine) = self.engine.clone() else {
            self.shell.state.status = "The AI Agent needs an open workspace".into();
            return;
        };
        if self.agent.loading {
            self.shell.state.status = "The agent chat is still loading; send again in a moment".into();
            return;
        }
        let automation = match self.agent_automation(&engine) {
            Ok(automation) => automation,
            Err(err) => {
                self.agent.notice(format!("The AI Agent could not start: {err}"), true);
                self.rebuild_agent_rows();
                return;
            }
        };
        // With no chat loaded yet, the turn thread reports it first, so it arrives before the
        // turn's own messages on the same channel.
        let load_first = !self.agent.loaded;
        self.agent.loading = load_first;
        self.agent.last_input = text.clone();
        self.shell.agent.input.clear();
        self.shell.agent.busy = Some("Thinking…".into());
        let cancel = Cancel::new();
        self.agent.job = Some(cancel.clone());
        let sender = self.agent.sender();
        let ctx = ctx.clone();
        let started = spawn("postguy-agent", move || {
            if load_first {
                let _ = sender.send(AgentMsg::Loaded(engine.agent_chat().map_err(|err| err.to_string())));
            }
            let mut host = ChannelHost { sender: sender.clone(), ctx: ctx.clone(), cancel: cancel.clone() };
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| automation.agent_turn(&text, &mut host, &cancel))).unwrap_or_else(|_| {
                Err(TurnError { message: "The AI Agent stopped unexpectedly; your workspace and chat are kept.".into(), recorded: false })
            });
            let _ = sender.send(AgentMsg::Done(result));
            ctx.request_repaint();
        });
        if !started {
            self.agent.job = None;
            self.agent.loading = false;
            self.shell.agent.busy = None;
            self.shell.agent.input = std::mem::take(&mut self.agent.last_input);
            self.agent.notice("The AI Agent could not start: no thread could be started".into(), true);
            self.rebuild_agent_rows();
        }
    }

    /// The agent changed the workspace: reload every open editor without unsaved edits (their
    /// session drafts too, so they reload from the files). Dirty editors are kept.
    fn reload_after_agent_edit(&mut self) {
        self.reload_variable_editors();
        let mut dropped = Vec::new();
        let mut kept_dirty = self.detail_editors.values().any(|editor| editor.dirty) || self.pending_env_save.is_some();
        self.editors.retain(|id, editor| {
            if !editor.dirty {
                dropped.push(*id);
            }
            kept_dirty |= editor.dirty;
            editor.dirty
        });
        self.ws_editors.retain(|id, editor| {
            if !editor.dirty {
                dropped.push(*id);
            }
            kept_dirty |= editor.dirty;
            editor.dirty
        });
        self.grpc_editors.retain(|id, editor| {
            if !editor.dirty {
                dropped.push(*id);
            }
            kept_dirty |= editor.dirty;
            editor.dirty
        });
        for id in dropped {
            self.shell.tabs.drafts.remove(&id);
        }
        if kept_dirty {
            self.shell.state.status =
                "The AI Agent changed the workspace while a tab has unsaved edits; saving that tab overwrites the agent's change".into();
        }
    }

    /// Settings → AI Agent actions. Keychain calls run on worker threads.
    pub(crate) fn apply_ai_settings(&mut self, action: postguy_ui_egui::settings::SettingsAction) {
        use postguy_ui_egui::settings::SettingsAction;
        let Some(engine) = self.engine.clone() else {
            self.shell.settings.ai.key_status = AiKeyStatus::Failed("No workspace is open.".into());
            self.shell.settings.ai.message = Some(("Settings could not be saved: the workspace engine is not running".into(), true));
            return;
        };
        let provider = self.shell.settings.ai.provider;
        let sender = self.agent.sender();
        let ctx = self.agent.ctx.clone();
        match action {
            SettingsAction::CheckAiKey => {
                let started = spawn("postguy-agent-key", move || {
                    let _ = sender
                        .send(AgentMsg::KeyStatus(provider, engine.ai_api_key(provider).map(|key| key.is_some()).map_err(|err| err.to_string())));
                    ctx.request_repaint();
                });
                if !started {
                    self.shell.settings.ai.key_status = AiKeyStatus::Failed("The keychain check could not start.".into());
                }
            }
            SettingsAction::SaveAi => {
                let draft = &mut self.shell.settings.ai;
                let endpoint = match postguy_automation::agent::normalize_endpoint(&draft.endpoint) {
                    Ok(endpoint) => endpoint,
                    Err(err) => {
                        draft.message = Some((format!("Not saved: {err}"), true));
                        return;
                    }
                };
                draft.endpoint = endpoint.clone();
                let mut settings = self.shell.settings_value.clone();
                settings.ai.provider = draft.provider;
                settings.ai.openai_endpoint = endpoint;
                settings.ai.models = draft.models.clone();
                if let Err(err) = engine.update_settings(settings.clone()) {
                    draft.message = Some((format!("Settings save failed: {err}"), true));
                    return;
                }
                self.shell.settings_value = settings;
                let key = draft.api_key.trim().to_string();
                if key.is_empty() {
                    draft.message = Some(("Saved".to_string(), false));
                    return;
                }
                draft.saving = true;
                draft.message = None;
                let started = spawn("postguy-agent-key", move || {
                    let result = engine
                        .set_ai_api_key(provider, Some(&key))
                        .map(|()| "Saved".to_string())
                        .map_err(|err| format!("Settings saved, but the API key was not stored: {err}"));
                    let _ = sender.send(AgentMsg::KeySaved(result));
                    ctx.request_repaint();
                });
                if !started {
                    let draft = &mut self.shell.settings.ai;
                    draft.saving = false;
                    draft.message = Some(("Settings saved, but the API key was not stored: no thread could be started".into(), true));
                }
            }
            SettingsAction::RemoveAiKey => {
                self.shell.settings.ai.saving = true;
                let started = spawn("postguy-agent-key", move || {
                    let _ = sender.send(AgentMsg::KeyRemoved(engine.set_ai_api_key(provider, None).map_err(|err| err.to_string())));
                    ctx.request_repaint();
                });
                if !started {
                    let draft = &mut self.shell.settings.ai;
                    draft.saving = false;
                    draft.message = Some(("The API key could not be removed: no thread could be started".into(), true));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn examples_name_real_collections_and_requests() {
        assert_eq!(examples(&[]).len(), 2, "an empty workspace gets the create examples");
        let request = postguy_model::Request { name: "List users".into(), ..Default::default() };
        let folder = postguy_model::Folder { name: "Users".into(), items: vec![Item::Request(Box::new(request))], ..Default::default() };
        let collection = Collection { name: "Payments API!".into(), items: vec![Item::Folder(Box::new(folder))], ..Default::default() };
        let out = examples(std::slice::from_ref(&collection));
        assert_eq!(out[0], "Run collection \"Payments API!\" and export the result to payments-api-results.csv");
        assert_eq!(out[2], "Fix request \"List users\" in collection \"Payments API!\"");
        assert_eq!(out.len(), 5);
        let empty = Collection { name: "Empty".into(), ..Default::default() };
        assert_eq!(examples(&[empty]).len(), 3, "no requests: no fix or test examples");
    }
}
