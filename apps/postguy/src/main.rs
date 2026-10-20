//! Postguy desktop app (eframe + glow): owns the engine session and applies the sidebar's
//! intents (M3.2). Launch with `--workspace DIR` to choose the workspace; without it the app
//! opens `./postguy-workspace` so a fresh checkout has somewhere to write.
//!
//! The UI thread never does network I/O; M3.2 has no sends. Reads of the workspace happen when
//! an action changed it (one revision), not every frame.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use egui::{Align, Layout, vec2};
use postguy_engine::{Cancel, CollectionFormat, Engine, ImportKind, ImportOptions, ImportOutcome, SendOptions, SendTarget};
use postguy_model::{Collection, Environment, HttpRequest, Id, Item, Request, RequestKind};
use postguy_store::{StorePaths, config_dir};
use postguy_ui_egui::builder::{BodyDrafts, BuilderAction, BuilderTab, RequestBuilder, SendProgress};
use postguy_ui_egui::collection::{CollectionAction, CollectionDetail, DetailState};
use postguy_ui_egui::commands::CommandAction;
use postguy_ui_egui::console::ConsoleRow;
use postguy_ui_egui::cookies::{CookieAction, CookieData, CookieRow, cookie_manager};
use postguy_ui_egui::environment::{EnvironmentAction, EnvironmentEditor};
use postguy_ui_egui::export::{ExportFormat, ExportRequest, ExportState, ExportTarget};
use postguy_ui_egui::import::{ImportAction, ImportItemView, ImportState, ImportSummary};
use postguy_ui_egui::response::{ResponseAction, ResponseData, ResponseViewerState, response_view};
use postguy_ui_egui::settings::SettingsAction;
use postguy_ui_egui::shell::PanelAction;
use postguy_ui_egui::sidebar::{RequestProtocol, RowKind, SidebarAction, SidebarData};
use postguy_ui_egui::tabs::{TabAction, TabDoc, Tabs};
use postguy_ui_egui::trash::{TrashAction, TrashEntryView, trash_view};
use postguy_ui_egui::widgets::field::VariableInfo;
use postguy_ui_egui::{PostguyShell, ThemeMode};
use serde::{Deserialize, Serialize};

mod control;

struct ClipboardItem {
    source_collection: Id,
    item: Item,
}

struct Clipboard {
    items: Vec<ClipboardItem>,
    cut: bool,
}

/// What the session file stores (M3.3). Tabs carry their own drafts; the rest is layout.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    #[serde(default)]
    tabs: Tabs,
    #[serde(default)]
    sidebar_sections: [bool; 3],
    #[serde(default)]
    sidebar_heights: [f32; 3],
    #[serde(default)]
    theme: String,
    /// False while the app is running; set true on a clean exit (crash recovery).
    #[serde(default)]
    clean_exit: bool,
}

/// A builder draft per request tab; names, unsaved fields and per-mode body data survive the
/// session through this shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEditor {
    request: Request,
    draft: HttpRequest,
    description: String,
    bodies: BodyDrafts,
    tab: BuilderTab,
}

struct RequestEditor {
    collection: String,
    request: Request,
    draft: HttpRequest,
    description: String,
    bodies: BodyDrafts,
    dirty: bool,
    tab: BuilderTab,
    /// The draft before the last cURL import (the inline Undo hint).
    undo_paste: Option<HttpRequest>,
    /// The auth kind inherited through folders/collection (None = no auth).
    inherited_auth: Option<String>,
}

struct DetailEditorState {
    collection: Collection,
    folder: Option<Id>,
    state: DetailState,
    dirty: bool,
}

struct EnvironmentEditorState {
    environment: Environment,
    globals: bool,
    search: String,
    show_description: bool,
    reveal: bool,
}

#[derive(Clone)]
enum RenameTarget {
    Collection(Id),
    Environment(Id),
    Item { collection: Id, id: Id },
}

#[derive(Clone)]
enum UndoEntry {
    Rename { target: RenameTarget, old: String, new: String },
    Move { collection: Id, id: Id, from_parent: Option<Id>, from_index: usize, to_parent: Option<Id>, to_index: usize },
}

struct ImportJob {
    receiver: std::sync::mpsc::Receiver<ImportSummary>,
}

/// Requests a repaint when the native file dialog future completes (it polls on the UI thread).
struct RepaintWake(egui::Context);

impl std::task::Wake for RepaintWake {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.request_repaint();
    }
}

struct PendingScreenshot {
    id: u64,
    path: PathBuf,
    reply: std::sync::mpsc::Sender<Result<serde_json::Value, String>>,
    started: std::time::Instant,
}

struct Sending {
    tab: u64,
    started: std::time::Instant,
    cancel: Cancel,
    download: bool,
    receiver: std::sync::mpsc::Receiver<Result<(String, u16, u64, u64), String>>,
}

struct App {
    engine: Option<Engine>,
    shell: PostguyShell,
    clipboard: Option<Clipboard>,
    editors: BTreeMap<u64, RequestEditor>,
    env_editors: BTreeMap<u64, EnvironmentEditorState>,
    pending_env_save: Option<(u64, Environment, bool, std::time::Instant)>,
    detail_editors: BTreeMap<u64, DetailEditorState>,
    pending_detail_save: Option<(u64, Collection, std::time::Instant)>,
    responses: BTreeMap<u64, ResponseData>,
    viewer_states: BTreeMap<u64, ResponseViewerState>,
    trash_entries: Vec<TrashEntryView>,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    sending: Option<Sending>,
    revision: u64,
    last_session: Option<String>,
    last_save: std::time::Instant,
    control_server: Option<control::ControlServer>,
    control_queue: Option<control::ControlQueue>,
    pending_screenshot: Option<PendingScreenshot>,
    import: ImportState,
    import_job: Option<ImportJob>,
    import_picker: Option<std::pin::Pin<Box<dyn std::future::Future<Output = Vec<PathBuf>>>>>,
    export: ExportState,
    export_request: Option<ExportRequest>,
    export_save: Option<std::pin::Pin<Box<dyn std::future::Future<Output = Option<PathBuf>>>>>,
    export_job: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    pending_clipboard: Option<String>,
}

impl App {
    fn new(workspace: PathBuf) -> App {
        let config = workspace.join(".postguy-config");
        let mut shell = PostguyShell::new(workspace.file_name().and_then(|name| name.to_str()).unwrap_or("Workspace"));
        match open_engine(&workspace, &config) {
            Ok(engine) => {
                let mut app = App {
                    engine: Some(engine),
                    shell,
                    clipboard: None,
                    editors: BTreeMap::new(),
                    env_editors: BTreeMap::new(),
                    pending_env_save: None,
                    detail_editors: BTreeMap::new(),
                    pending_detail_save: None,
                    responses: BTreeMap::new(),
                    viewer_states: BTreeMap::new(),
                    trash_entries: Vec::new(),
                    undo_stack: Vec::new(),
                    redo_stack: Vec::new(),
                    sending: None,
                    revision: 0,
                    last_session: None,
                    last_save: std::time::Instant::now(),
                    control_server: None,
                    control_queue: None,
                    pending_screenshot: None,
                    import: ImportState::default(),
                    import_job: None,
                    import_picker: None,
                    export: ExportState::default(),
                    export_request: None,
                    export_save: None,
                    export_job: None,
                    pending_clipboard: None,
                };
                app.shell.settings_value = app.engine.as_ref().map(|engine| engine.settings()).unwrap_or_default();
                app.refresh();
                app.restore_session();
                app
            }
            Err(message) => {
                shell.state.status = message.clone();
                App {
                    engine: None,
                    shell,
                    clipboard: None,
                    editors: BTreeMap::new(),
                    env_editors: BTreeMap::new(),
                    pending_env_save: None,
                    detail_editors: BTreeMap::new(),
                    pending_detail_save: None,
                    responses: BTreeMap::new(),
                    viewer_states: BTreeMap::new(),
                    trash_entries: Vec::new(),
                    undo_stack: Vec::new(),
                    redo_stack: Vec::new(),
                    sending: None,
                    revision: 0,
                    last_session: None,
                    last_save: std::time::Instant::now(),
                    control_server: None,
                    control_queue: None,
                    pending_screenshot: None,
                    import: ImportState::default(),
                    import_job: None,
                    import_picker: None,
                    export: ExportState::default(),
                    export_request: None,
                    export_save: None,
                    export_job: None,
                    pending_clipboard: None,
                }
            }
        }
    }

    /// Start answering the opt-in control channel (AGENTS.md 3). Only `--control` calls this.
    fn attach_control(&mut self, server: control::ControlServer) {
        self.control_queue = Some(server.queue());
        self.control_server = Some(server);
    }

    /// Drain control requests at the top of a frame, then capture a requested screenshot once
    /// egui delivers it. The network thread only queues; all UI work stays on this thread.
    fn poll_control(&mut self, ctx: &egui::Context) {
        if let Some(server) = &self.control_server {
            server.set_context(ctx.clone());
        }
        if self.pending_screenshot.as_ref().is_some_and(|pending| pending.started.elapsed() > std::time::Duration::from_secs(10))
            && let Some(pending) = self.pending_screenshot.take()
        {
            let _ = pending.reply.send(Err("the screenshot did not arrive from the windowing system".into()));
        }
        self.capture_screenshot(ctx);
        let Some(queue) = self.control_queue.clone() else { return };
        let requests: Vec<_> = match queue.lock() {
            Ok(mut queue) => queue.drain(..).collect(),
            Err(_) => return,
        };
        for (command, reply) in requests {
            if let control::ControlCommand::Screenshot(path) = command {
                if let Err(error) = self.begin_screenshot(path, reply.clone(), ctx) {
                    let _ = reply.send(Err(error));
                }
                continue;
            }
            let result = self.apply_control(command);
            let _ = reply.send(result);
        }
    }

    fn apply_control(&mut self, command: control::ControlCommand) -> Result<serde_json::Value, String> {
        match command {
            control::ControlCommand::Inspect => Ok(self.inspect()),
            control::ControlCommand::Command(id) => {
                let registry = postguy_ui_egui::commands::CommandRegistry::default();
                let action =
                    registry.action(&id).ok_or_else(|| format!("unknown command `{id}`; run with `--help` or inspect the command palette"))?;
                self.apply_command(action);
                Ok(serde_json::json!({"ok": true, "command": id}))
            }
            control::ControlCommand::Set(key, value) => self.set_control(&key, value),
            // Handled in `poll_control`, which keeps the reply channel until the frame is captured.
            control::ControlCommand::Screenshot(_) => Err("internal screenshot handling".into()),
            control::ControlCommand::Click(_, _) => Err("synthetic clicks are not supported; use `command` or `set`".into()),
            control::ControlCommand::Key(_) => Err("synthetic keys are not supported; use `command` or `set`".into()),
            control::ControlCommand::Type(_) => Err("synthetic typing is not supported; use `command` or `set`".into()),
        }
    }

    fn begin_screenshot(
        &mut self,
        path: PathBuf,
        reply: std::sync::mpsc::Sender<Result<serde_json::Value, String>>,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        if self.pending_screenshot.is_some() {
            return Err("a screenshot is already in progress".into());
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|err| format!("creating {}: {err}", parent.display()))?;
        }
        let id = self.revision.wrapping_add(1);
        self.pending_screenshot = Some(PendingScreenshot { id, path, reply, started: std::time::Instant::now() });
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(id)));
        Ok(())
    }

    fn set_control(&mut self, key: &str, value: serde_json::Value) -> Result<serde_json::Value, String> {
        match key {
            "tab" => {
                let name = value.as_str().ok_or("`tab` needs a string value")?;
                let doc = match name {
                    "settings" => TabDoc::Settings,
                    "globals" => TabDoc::Globals,
                    "cookies" => TabDoc::Cookies,
                    "trash" => TabDoc::Trash,
                    "runner" => TabDoc::Runner,
                    other => {
                        let tab = self
                            .shell
                            .tabs
                            .tabs
                            .iter()
                            .find(|tab| tab.title.eq_ignore_ascii_case(other))
                            .ok_or_else(|| format!("unknown tab `{other}`"))?;
                        self.shell.tabs.activate(tab.id);
                        return Ok(serde_json::json!({"ok": true, "tab": other}));
                    }
                };
                let title = match &doc {
                    TabDoc::Settings => "Settings",
                    TabDoc::Globals => "Globals",
                    TabDoc::Cookies => "Cookies",
                    TabDoc::Trash => "Trash",
                    TabDoc::Runner => "Runner",
                    _ => "Tab",
                };
                self.shell.tabs.open(doc, title);
                Ok(serde_json::json!({"ok": true, "tab": name}))
            }
            "console" | "right" | "two-pane" | "sidebar" => {
                let open = value.as_bool().ok_or("panel values are booleans")?;
                match key {
                    "console" => self.shell.state.console_open = open,
                    "right" => self.shell.state.right_panel_open = open,
                    "two-pane" => self.shell.state.two_pane = open,
                    _ => self.shell.state.sidebar_open = open,
                }
                Ok(serde_json::json!({"ok": true, "key": key, "value": open}))
            }
            "theme" => {
                let mode = match value.as_str() {
                    Some("light") => ThemeMode::Light,
                    Some("dark") => ThemeMode::Dark,
                    Some("system") => ThemeMode::System,
                    _ => return Err("`theme` is light, dark or system".into()),
                };
                self.shell.state.theme_mode = mode;
                Ok(serde_json::json!({"ok": true, "theme": value}))
            }
            "url" => {
                let url = value.as_str().ok_or("`url` needs a string value")?;
                let active = self.shell.tabs.active.ok_or("no active tab")?;
                let editor = self.editors.get_mut(&active).ok_or("the active tab has no request editor")?;
                editor.draft.url = url.to_string();
                editor.dirty = true;
                Ok(serde_json::json!({"ok": true, "url": url}))
            }
            other => Err(format!("unknown key `{other}`; keys are tab, console, right, two-pane, sidebar, theme, url")),
        }
    }

    fn inspect(&self) -> serde_json::Value {
        let tabs: Vec<serde_json::Value> =
            self.shell.tabs.tabs.iter().map(|tab| serde_json::json!({"id": tab.id, "title": tab.title, "dirty": tab.dirty})).collect();
        let panel = self.shell.tabs.active_tab().map(|tab| match &tab.doc {
            TabDoc::Request { .. } => "request",
            TabDoc::Environment { .. } | TabDoc::Globals => "environment",
            TabDoc::Collection { .. } | TabDoc::Folder { .. } => "collection",
            TabDoc::Settings => "settings",
            TabDoc::Cookies => "cookies",
            TabDoc::Trash => "trash",
            TabDoc::Runner => "runner",
            TabDoc::History { .. } => "history",
            TabDoc::Welcome => "welcome",
        });
        serde_json::json!({
            "tabs": tabs,
            "active": self.shell.tabs.active,
            "panel": panel,
            "sidebar": self.shell.state.sidebar_open,
            "console": self.shell.state.console_open,
            "right": self.shell.state.right_panel_open,
            "twoPane": self.shell.state.two_pane,
            "status": self.shell.state.status,
            "consoleRows": self.shell.console_rows.len(),
            "engine": self.engine.is_some(),
        })
    }

    fn open_export(&mut self, target: ExportTarget) {
        postguy_ui_egui::export::open(&mut self.export, target);
    }

    fn begin_export(&mut self, request: ExportRequest) {
        if self.export_save.is_some() || self.export_job.is_some() {
            self.export.error = Some("an export is already running".into());
            return;
        }
        self.export.busy = true;
        self.export.error = None;
        self.export.done = None;
        self.export_request = Some(request.clone());
        self.export_save = Some(postguy_platform::save_export(request.default_name, request.extension));
    }

    fn poll_export(&mut self, ctx: &egui::Context) {
        self.poll_export_save(ctx);
        if let Some(receiver) = self.export_job.take() {
            match receiver.try_recv() {
                Ok(result) => {
                    self.export.busy = false;
                    match result {
                        Ok(message) => {
                            self.export.done = Some(message.clone());
                            self.shell.state.status = message;
                        }
                        Err(error) => self.export.error = Some(error),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => self.export_job = Some(receiver),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.export.busy = false;
                    self.export.error = Some("the export worker stopped without a result".into());
                }
            }
        }
    }

    fn poll_export_save(&mut self, ctx: &egui::Context) {
        let Some(mut picker) = self.export_save.take() else { return };
        let waker = std::task::Waker::from(std::sync::Arc::new(RepaintWake(ctx.clone())));
        let mut task = std::task::Context::from_waker(&waker);
        match picker.as_mut().poll(&mut task) {
            std::task::Poll::Pending => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                self.export_save = Some(picker);
            }
            std::task::Poll::Ready(path) => match path {
                Some(path) => {
                    if let Some(request) = self.export_request.take() {
                        self.start_export(request, path);
                    }
                }
                None => {
                    self.export_request = None;
                    self.export.busy = false;
                }
            },
        }
    }

    fn start_export(&mut self, request: ExportRequest, path: PathBuf) {
        let Some(engine) = self.engine.clone() else {
            self.export.busy = false;
            self.export.error = Some("no workspace is open".into());
            return;
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        self.export_job = Some(receiver);
        std::thread::Builder::new()
            .name("postguy-export".into())
            .spawn(move || {
                let result = (|| -> Result<String, String> {
                    match request.target.clone() {
                        ExportTarget::Collection { id, name } => {
                            let format = match request.format {
                                ExportFormat::Postman => CollectionFormat::Postman,
                                ExportFormat::Native => CollectionFormat::Native,
                                ExportFormat::Zip => return Err("a collection cannot be exported as a workspace zip".into()),
                            };
                            let value = engine.export_collection_value(&postguy_model::Id(id), format).map_err(|err| err.to_string())?;
                            let bytes = serde_json::to_vec_pretty(&value).map_err(|err| err.to_string())?;
                            postguy_util::atomic_write(&path, &bytes).map_err(|err| err.to_string())?;
                            Ok(format!("Exported `{name}` to {}", path.display()))
                        }
                        ExportTarget::Environment { id, name } => {
                            let value =
                                engine.export_environment_value(&postguy_model::Id(id), request.include_secrets).map_err(|err| err.to_string())?;
                            let bytes = serde_json::to_vec_pretty(&value).map_err(|err| err.to_string())?;
                            postguy_util::atomic_write(&path, &bytes).map_err(|err| err.to_string())?;
                            Ok(format!("Exported environment `{name}` to {}", path.display()))
                        }
                        ExportTarget::Globals => {
                            let value = engine.export_globals_value(request.include_secrets).map_err(|err| err.to_string())?;
                            let bytes = serde_json::to_vec_pretty(&value).map_err(|err| err.to_string())?;
                            postguy_util::atomic_write(&path, &bytes).map_err(|err| err.to_string())?;
                            Ok(format!("Exported globals to {}", path.display()))
                        }
                        ExportTarget::Workspace => {
                            let bytes = engine.export_workspace_zip(&path).map_err(|err| err.to_string())?;
                            Ok(format!("Exported the workspace ({bytes} bytes) to {}", path.display()))
                        }
                    }
                })();
                let _ = sender.send(result);
            })
            .ok();
    }

    fn open_import(&mut self) {
        postguy_ui_egui::import::open(&mut self.import);
    }

    fn apply_import_action(&mut self, action: ImportAction) {
        match action {
            ImportAction::ImportText { text, replace } => self.start_import_text(text, replace),
            ImportAction::PickFiles { folder } => {
                if self.import_picker.is_none() {
                    self.import.error = None;
                    self.import_picker = Some(postguy_platform::pick_import_paths(folder));
                }
            }
            ImportAction::Open { index } => self.open_imported(index),
        }
    }

    fn start_import_text(&mut self, text: String, replace: bool) {
        let Some(engine) = self.engine.clone() else {
            self.import.error = Some("no workspace is open".into());
            return;
        };
        if self.import_job.is_some() {
            self.import.error = Some("an import is already running".into());
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        self.import.busy = true;
        self.import.error = None;
        self.import.summary = None;
        self.import_job = Some(ImportJob { receiver });
        std::thread::Builder::new()
            .name("postguy-import".into())
            .spawn(move || {
                let summary = match engine.import_text(&text, ImportOptions { replace }) {
                    Ok(outcome) => ImportSummary { items: vec![import_item("Pasted text", outcome)], problems: Vec::new() },
                    Err(err) => ImportSummary { items: Vec::new(), problems: vec![err.to_string()] },
                };
                let _ = sender.send(summary);
            })
            .ok();
    }

    fn start_import_paths(&mut self, paths: Vec<PathBuf>) {
        let Some(engine) = self.engine.clone() else {
            self.import.error = Some("no workspace is open".into());
            return;
        };
        if self.import_job.is_some() {
            self.import.error = Some("an import is already running".into());
            return;
        }
        let replace = self.import.replace;
        let (sender, receiver) = std::sync::mpsc::channel();
        self.import.busy = true;
        self.import.error = None;
        self.import.summary = None;
        self.import_job = Some(ImportJob { receiver });
        std::thread::Builder::new()
            .name("postguy-import".into())
            .spawn(move || {
                let (files, problems) = postguy_engine::walk_import_paths(paths);
                let mut summary = ImportSummary { items: Vec::new(), problems };
                for path in files {
                    match engine.import_file(&path, ImportOptions { replace }) {
                        Ok(outcome) => summary.items.push(import_item(&path.display().to_string(), outcome)),
                        Err(err) => summary.items.push(ImportItemView {
                            source: path.display().to_string(),
                            error: Some(err.to_string()),
                            ..ImportItemView::default()
                        }),
                    }
                }
                let _ = sender.send(summary);
            })
            .ok();
    }

    fn poll_import_picker(&mut self, ctx: &egui::Context) {
        let Some(mut picker) = self.import_picker.take() else { return };
        let waker = std::task::Waker::from(std::sync::Arc::new(RepaintWake(ctx.clone())));
        let mut task = std::task::Context::from_waker(&waker);
        match picker.as_mut().poll(&mut task) {
            std::task::Poll::Pending => self.import_picker = Some(picker),
            std::task::Poll::Ready(paths) => {
                if !paths.is_empty() {
                    self.start_import_paths(paths);
                }
            }
        }
    }

    fn poll_import(&mut self) {
        let Some(job) = self.import_job.take() else { return };
        match job.receiver.try_recv() {
            Ok(summary) => {
                self.import.busy = false;
                self.import.error = None;
                self.import.summary = Some(summary);
                self.refresh();
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => self.import_job = Some(job),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.import.busy = false;
                self.import.error = Some("the import worker stopped without a result".into());
            }
        }
    }

    fn open_imported(&mut self, index: usize) {
        let Some(item) = self.import.summary.as_ref().and_then(|summary| summary.items.get(index)).cloned() else { return };
        if item.error.is_some() {
            return;
        }
        match item.kind.as_str() {
            "collection" => {
                if let Some(id) = item.id {
                    self.shell.tabs.open(TabDoc::Collection { id }, item.name);
                }
            }
            "environment" => {
                if let Some(id) = item.id {
                    self.shell.tabs.open(TabDoc::Environment { id }, item.name);
                }
            }
            "globals" => {
                self.shell.tabs.open(TabDoc::Globals, "Globals");
            }
            _ => {}
        }
        self.import.open = false;
    }

    fn capture_screenshot(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_screenshot.take() else { return };
        let screenshot = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { user_data, image, .. } => {
                    let id = user_data.data.as_ref().and_then(|data| data.downcast_ref::<u64>()).copied();
                    Some((id, image.clone()))
                }
                _ => None,
            })
        });
        let Some((id, image)) = screenshot else {
            self.pending_screenshot = Some(pending);
            return;
        };
        if id != Some(pending.id) {
            self.pending_screenshot = Some(pending);
            return;
        }
        let [width, height] = image.size;
        let mut rgba = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
        for pixel in &image.pixels {
            rgba.extend_from_slice(&pixel.to_array());
        }
        let result = image::save_buffer_with_format(
            &pending.path,
            &rgba,
            width as u32,
            height as u32,
            image::ExtendedColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|err| format!("writing {}: {err}", pending.path.display()));
        #[cfg(unix)]
        if result.is_ok() {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&pending.path, std::fs::Permissions::from_mode(0o600));
        }
        let _ = pending.reply.send(result.map(|()| serde_json::json!({"ok": true, "path": pending.path})));
    }

    fn refresh(&mut self) {
        let Some(engine) = &self.engine else { return };
        self.revision = self.revision.wrapping_add(1);
        let history = engine.history(200).unwrap_or_default();
        let environments = engine.environments().unwrap_or_default();
        let active = engine.active_environment().unwrap_or_default();
        let globals = engine.globals().unwrap_or_default();
        self.trash_entries = engine
            .trash_entries()
            .unwrap_or_default()
            .into_iter()
            .map(|entry| TrashEntryView {
                id: entry.id,
                kind: entry.kind,
                original: entry.original,
                deleted_at_ms: entry.deleted_at_ms,
                size_bytes: entry.size_bytes,
            })
            .collect();
        let mut data = SidebarData {
            revision: self.revision,
            collections: engine.collections().unwrap_or_default(),
            environments,
            history,
            active_environment: active,
            globals,
        };
        if data.environments.is_empty() {
            data.active_environment = None;
        }
        self.shell.sidebar_data = data;
        self.shell.console_rows = engine
            .console_entries(0)
            .into_iter()
            .map(|entry| ConsoleRow {
                id: entry.id,
                time_ms: entry.time_ms,
                level: match entry.level {
                    postguy_engine::console::ConsoleLevel::Info => "info".into(),
                    postguy_engine::console::ConsoleLevel::Warning => "warning".into(),
                    postguy_engine::console::ConsoleLevel::Error => "error".into(),
                },
                kind: match entry.kind {
                    postguy_engine::console::ConsoleKind::Request => "request".into(),
                    postguy_engine::console::ConsoleKind::Log => "log".into(),
                    postguy_engine::console::ConsoleKind::Warning => "warning".into(),
                    postguy_engine::console::ConsoleKind::Error => "error".into(),
                },
                message: entry.message,
                detail: entry.detail,
            })
            .collect();
        self.shell.cookie_data = CookieData {
            cookies: engine
                .cookies()
                .into_iter()
                .map(|cookie| CookieRow {
                    name: cookie.name,
                    value: cookie.value,
                    domain: cookie.domain,
                    path: cookie.path,
                    secure: cookie.secure,
                    http_only: cookie.http_only,
                    expires_ms: cookie.expires_ms,
                })
                .collect(),
            allowlist: engine.cookie_domains(),
        };
        let dropped = validate_tabs(&mut self.shell.tabs, &self.shell.sidebar_data);
        if dropped > 0 {
            self.shell.state.status = format!("Closed {dropped} tab(s) whose item no longer exists");
        }
    }

    /// Load tabs and layout from the store's session file (atomic writes; corrupt files are
    /// quarantined by the store). Missing documents are dropped, and an unclean exit is flagged.
    fn restore_session(&mut self) {
        let Some(engine) = &self.engine else { return };
        let Ok(session) = config_dir::load_session(engine.store_paths()) else { return };
        let Ok(stored) = serde_json::from_value::<StoredSession>(session.0.clone()) else { return };
        self.shell.tabs = stored.tabs;
        if stored.sidebar_heights.iter().all(|height| *height > 0.0) {
            self.shell.sidebar_state.section_heights = stored.sidebar_heights;
        }
        self.shell.sidebar_state.sections = stored.sidebar_sections;
        self.shell.state.theme_mode = match stored.theme.as_str() {
            "light" => ThemeMode::Light,
            "dark" => ThemeMode::Dark,
            _ => ThemeMode::System,
        };
        if !stored.clean_exit {
            self.shell.state.status = "Session restored after an unexpected exit".into();
        }
        self.refresh();
    }

    /// Persist tabs/layout when they changed (called at most once a second, plus on exit).
    fn save_session(&mut self, clean_exit: bool, force: bool) {
        let Some(engine) = &self.engine else { return };
        let theme = match self.shell.state.theme_mode {
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
            ThemeMode::System => "system",
        };
        for (tab_id, editor) in &self.editors {
            if !editor.dirty {
                continue;
            }
            let stored = StoredEditor {
                request: editor.request.clone(),
                draft: editor.draft.clone(),
                description: editor.description.clone(),
                bodies: editor.bodies.clone(),
                tab: editor.tab,
            };
            if let Ok(text) = serde_json::to_string(&stored) {
                self.shell.tabs.set_draft(*tab_id, text);
            }
        }
        let stored = StoredSession {
            tabs: self.shell.tabs.clone(),
            sidebar_sections: self.shell.sidebar_state.sections,
            sidebar_heights: self.shell.sidebar_state.section_heights,
            theme: theme.to_string(),
            clean_exit,
        };
        let Ok(json) = serde_json::to_string(&stored) else { return };
        if !force && self.last_session.as_deref() == Some(json.as_str()) {
            return;
        }
        if !force && self.last_save.elapsed() < std::time::Duration::from_secs(1) {
            return;
        }
        if config_dir::save_session(engine.store_paths(), &config_dir::SessionFile(serde_json::to_value(&stored).unwrap_or_default())).is_ok() {
            self.last_session = Some(json);
            self.last_save = std::time::Instant::now();
        }
    }

    fn apply(&mut self, action: SidebarAction) {
        let Some(engine) = self.engine.clone() else { return };
        let outcome = self.apply_inner(&engine, action);
        match outcome {
            Ok(status) => self.shell.state.status = status,
            Err(err) => self.shell.state.status = err,
        }
        self.refresh();
    }

    fn apply_inner(&mut self, engine: &Engine, action: SidebarAction) -> Result<String, String> {
        let collections = engine.collections().map_err(|err| err.to_string())?;
        match action {
            SidebarAction::Open { id, kind, .. } => Ok(format!("Opened {kind:?} {id}")),
            SidebarAction::OpenHistory { id } => {
                if let Some(entry) = engine.history(usize::MAX).map_err(|err| err.to_string())?.into_iter().find(|entry| entry.id.0 == id) {
                    let label = entry.request.as_ref().and_then(|request| match request {
                        RequestKind::Http(http) => Some(http.url.clone()),
                        _ => None,
                    });
                    if let Some(label) = label {
                        self.shell.tabs.open(TabDoc::History { id }, label);
                    }
                }
                Ok("Opened history entry".into())
            }
            SidebarAction::DeleteHistory { id } => {
                engine.delete_history(&id).map_err(|err| err.to_string())?;
                Ok("Deleted history entry".into())
            }
            SidebarAction::NewCollection => {
                let collection = engine.create_collection("New Collection").map_err(|err| err.to_string())?;
                Ok(format!("Created collection `{}`", collection.name))
            }
            SidebarAction::NewEnvironment => {
                let environment = engine.create_environment("New Environment").map_err(|err| err.to_string())?;
                Ok(format!("Created environment `{}`", environment.name))
            }
            SidebarAction::NewRequest { collection, parent, protocol } => {
                let protocol = match protocol {
                    RequestProtocol::Http => "http",
                    RequestProtocol::Grpc => "grpc",
                    RequestProtocol::WebSocket => "websocket",
                };
                let parent_id = parent.as_ref().map(|id| Id(id.clone()));
                let collection_id = Id(collection.clone());
                let request = engine.create_request(&collection_id, parent_id.as_ref(), protocol, "New Request").map_err(|err| err.to_string())?;
                self.shell.tabs.open(
                    TabDoc::Request { collection, request: request.id.0.clone(), method: "GET".into(), protocol: "http".into() },
                    request.name.clone(),
                );
                Ok("Created request".into())
            }
            SidebarAction::NewFolder { collection, parent } => {
                let parent_id = parent.as_ref().map(|id| Id(id.clone()));
                let folder = engine.create_folder(&Id(collection), parent_id.as_ref(), "New Folder").map_err(|err| err.to_string())?;
                Ok(format!("Created folder `{}`", folder.name))
            }
            SidebarAction::Rename { id, name } => {
                let id = Id(id.clone());
                let target = if collections.iter().any(|collection| collection.id == id) {
                    Some(RenameTarget::Collection(id.clone()))
                } else if engine.environments().map_err(|err| err.to_string())?.iter().any(|environment| environment.id == id) {
                    Some(RenameTarget::Environment(id.clone()))
                } else {
                    item_collection(&collections, &id).map(|collection| RenameTarget::Item { collection, id: id.clone() })
                };
                let old = target.as_ref().and_then(|target| match target {
                    RenameTarget::Collection(id) => {
                        collections.iter().find(|collection| collection.id == *id).map(|collection| collection.name.clone())
                    }
                    RenameTarget::Environment(id) => engine
                        .environments()
                        .ok()
                        .and_then(|environments| environments.into_iter().find(|environment| environment.id == *id))
                        .map(|environment| environment.name),
                    RenameTarget::Item { collection: collection_id, id } => collections
                        .iter()
                        .find(|collection| collection.id == *collection_id)
                        .and_then(|collection| find(&collection.items, id).map(|item| item.name().to_string())),
                });
                let Some(target) = target else { return Err(format!("`{}` is not in the workspace", id.0)) };
                match &target {
                    RenameTarget::Collection(id) => engine.rename_collection(id, &name).map_err(|err| err.to_string())?,
                    RenameTarget::Environment(id) => engine.rename_environment(id, &name).map_err(|err| err.to_string())?,
                    RenameTarget::Item { collection, id } => engine.rename_item(collection, id, &name).map_err(|err| err.to_string())?,
                }
                if let Some(old) = old {
                    self.undo_stack.push(UndoEntry::Rename { target, old, new: name });
                    self.redo_stack.clear();
                }
                Ok("Renamed".into())
            }
            SidebarAction::Delete { id, kind } => {
                let id = Id(id.clone());
                match kind {
                    RowKind::Collection => engine.delete_collection(&id).map_err(|err| err.to_string())?,
                    RowKind::Environment => engine.delete_environment(&id).map_err(|err| err.to_string())?,
                    _ => {
                        let collection = item_collection(&collections, &id).ok_or_else(|| format!("item `{}` is not in the workspace", id.0))?;
                        engine.delete_item(&collection, &id).map_err(|err| err.to_string())?;
                    }
                }
                Ok("Moved to trash".into())
            }
            SidebarAction::DeleteMany { items } => {
                let mut deleted = 0usize;
                for (id, kind) in items {
                    let id = Id(id);
                    match kind {
                        RowKind::Collection => engine.delete_collection(&id).map_err(|err| err.to_string())?,
                        RowKind::Environment => engine.delete_environment(&id).map_err(|err| err.to_string())?,
                        _ => {
                            let collection = item_collection(&collections, &id).ok_or_else(|| format!("item `{}` is not in the workspace", id.0))?;
                            engine.delete_item(&collection, &id).map_err(|err| err.to_string())?;
                        }
                    }
                    deleted += 1;
                }
                Ok(format!("Moved {deleted} items to the trash"))
            }
            SidebarAction::Duplicate { id, kind } => {
                let id = Id(id.clone());
                match kind {
                    RowKind::Collection => {
                        engine.duplicate_collection(&id, None).map_err(|err| err.to_string())?;
                    }
                    RowKind::Environment => {
                        engine.duplicate_environment(&id, None).map_err(|err| err.to_string())?;
                    }
                    _ => {
                        let collection = item_collection(&collections, &id).ok_or_else(|| format!("item `{}` is not in the workspace", id.0))?;
                        engine.duplicate_item(&collection, &id).map_err(|err| err.to_string())?;
                    }
                }
                Ok("Duplicated".into())
            }
            SidebarAction::MoveMany { ids, parent, index, copy } => {
                let mut moved = 0usize;
                let mut target_collection: Option<Id> = None;
                for item in ids {
                    let id = Id(item);
                    let collection = item_collection(&collections, &id).ok_or_else(|| format!("item `{}` is not in the workspace", id.0))?;
                    if let Some(expected) = &target_collection
                        && *expected != collection
                    {
                        return Err("a bulk move must stay inside one collection".into());
                    }
                    target_collection = Some(collection.clone());
                    let parent_id = parent.as_ref().map(|value| Id(value.clone()));
                    engine.move_item(&collection, &id, parent_id.as_ref(), index.saturating_add(moved), copy).map_err(|err| err.to_string())?;
                    moved += 1;
                }
                Ok(format!("Moved {moved} items"))
            }
            SidebarAction::Move { id, parent, index, copy } => {
                let id = Id(id.clone());
                let collection = item_collection(&collections, &id).ok_or_else(|| format!("item `{}` is not in the workspace", id.0))?;
                let from = locate_item(&collections, &id);
                let parent_id = parent.as_ref().map(|id| Id(id.clone()));
                engine.move_item(&collection, &id, parent_id.as_ref(), index, copy).map_err(|err| err.to_string())?;
                if !copy && let Some((from_parent, from_index)) = from {
                    self.undo_stack.push(UndoEntry::Move {
                        collection: collection.clone(),
                        id,
                        from_parent,
                        from_index,
                        to_parent: parent_id,
                        to_index: index,
                    });
                    self.redo_stack.clear();
                }
                Ok(if copy { "Copied".into() } else { "Moved".into() })
            }
            SidebarAction::Copy { id } => {
                let id = Id(id.clone());
                if let Some((source_collection, item)) = item_with_collection(&collections, &id) {
                    self.clipboard = Some(Clipboard { items: vec![ClipboardItem { source_collection, item }], cut: false });
                    Ok("Copied to the clipboard".into())
                } else {
                    Err("copying whole collections arrives with export (M4)".into())
                }
            }
            SidebarAction::Cut { id } => {
                let id = Id(id.clone());
                if let Some((source_collection, item)) = item_with_collection(&collections, &id) {
                    self.clipboard = Some(Clipboard { items: vec![ClipboardItem { source_collection, item }], cut: true });
                    Ok("Cut to the clipboard".into())
                } else {
                    Err("cutting whole collections is not supported".into())
                }
            }
            SidebarAction::Paste { target } => {
                let Some(clipboard) = self.clipboard.take() else { return Err("the clipboard is empty".into()) };
                let target = Id(target);
                let (target_collection, parent) = resolve_target(&collections, &target)?;
                let mut index = 0usize;
                let count = clipboard.items.len();
                for entry in clipboard.items {
                    if clipboard.cut && entry.source_collection == target_collection {
                        engine
                            .move_item(&target_collection, &entry.item.id().clone(), parent.as_ref(), index, false)
                            .map_err(|err| err.to_string())?;
                    } else {
                        engine.paste_item(&target_collection, parent.as_ref(), entry.item.clone(), true).map_err(|err| err.to_string())?;
                        if clipboard.cut {
                            engine.delete_item(&entry.source_collection, &entry.item.id().clone()).map_err(|err| err.to_string())?;
                        }
                    }
                    index = index.saturating_add(1);
                }
                Ok(match (count, clipboard.cut) {
                    (1, true) => "Moved".into(),
                    (1, false) => "Pasted".into(),
                    (n, true) => format!("Moved {n} items"),
                    (n, false) => format!("Pasted {n} items"),
                })
            }
            SidebarAction::CopyMany { ids } => {
                let mut items = Vec::new();
                for id in ids {
                    let id = Id(id);
                    if let Some((source_collection, item)) = item_with_collection(&collections, &id) {
                        items.push(ClipboardItem { source_collection, item });
                    }
                }
                if items.is_empty() {
                    return Err("nothing copyable is selected".into());
                }
                let count = items.len();
                self.clipboard = Some(Clipboard { items, cut: false });
                Ok(format!("Copied {count} items"))
            }
            SidebarAction::CutMany { ids } => {
                let mut items = Vec::new();
                for id in ids {
                    let id = Id(id);
                    if let Some((source_collection, item)) = item_with_collection(&collections, &id) {
                        items.push(ClipboardItem { source_collection, item });
                    }
                }
                if items.is_empty() {
                    return Err("nothing cuttable is selected".into());
                }
                let count = items.len();
                self.clipboard = Some(Clipboard { items, cut: true });
                Ok(format!("Cut {count} items"))
            }
            SidebarAction::SetActiveEnvironment(id) => {
                let label = id.clone();
                engine.set_active_environment(id.map(Id)).map_err(|err| err.to_string())?;
                Ok(match label {
                    Some(id) => format!("Active environment: {id}"),
                    None => "No active environment".into(),
                })
            }
            SidebarAction::CreateExample { id } => {
                let id = Id(id);
                let collection = item_collection(&collections, &id).ok_or_else(|| format!("request `{}` is not in the workspace", id.0))?;
                let example = engine.create_example(&collection, &id, "Example").map_err(|err| err.to_string())?;
                Ok(format!("Created example `{}`", example.name))
            }
            SidebarAction::Run { .. } => Ok("The runner arrives in M3.8/M9".into()),
            SidebarAction::Export { id } => {
                let id = Id(id.clone());
                if let Ok(collection) = engine.collection(&id) {
                    self.open_export(ExportTarget::Collection { id: collection.id.0, name: collection.name });
                } else if let Ok(environment) = engine.environment(&id) {
                    self.open_export(ExportTarget::Environment { id: environment.id.0, name: environment.name });
                } else {
                    return Err(format!("`{}` is not a collection or environment", id.0));
                }
                Ok("Export…".into())
            }
            SidebarAction::CopyAsCurl { id } => {
                let id = Id(id.clone());
                let Some((_collection, item)) = item_with_collection(&collections, &id) else {
                    return Err(format!("`{}` is not a request", id.0));
                };
                let postguy_model::Item::Request(request) = item else { return Err("Copy as cURL applies to requests".into()) };
                let postguy_model::RequestKind::Http(http) = &request.kind else {
                    return Err("Copy as cURL supports HTTP requests in M4".into());
                };
                self.pending_clipboard =
                    Some(postguy_snippets::generate(postguy_snippets::Language::Curl, http, postguy_snippets::Options::default()));
                Ok("Copied as cURL".into())
            }
            SidebarAction::OpenInNewTab { id, .. } => Ok(format!("Open in new tab: {id} (tab model arrives in M3.3)")),
            SidebarAction::Import => {
                self.open_import();
                Ok("Import…".into())
            }
        }
    }
}

impl App {
    /// Load builders for request tabs that do not have one yet, drop editors for closed tabs,
    /// and prefer a restored session draft over the file (crash recovery).
    fn sync_editors(&mut self) {
        let Some(engine) = self.engine.clone() else { return };
        let request_tabs: Vec<(u64, String, String)> = self
            .shell
            .tabs
            .tabs
            .iter()
            .filter_map(|tab| match &tab.doc {
                TabDoc::Request { collection, request, .. } => Some((tab.id, collection.clone(), request.clone())),
                _ => None,
            })
            .collect();
        self.editors.retain(|id, _| request_tabs.iter().any(|(tab, _, _)| tab == id));
        self.responses.retain(|id, _| request_tabs.iter().any(|(tab, _, _)| tab == id));
        self.viewer_states.retain(|id, _| request_tabs.iter().any(|(tab, _, _)| tab == id));
        for (tab_id, collection, request_id) in request_tabs {
            if self.editors.contains_key(&tab_id) {
                continue;
            }
            let stored = self.shell.tabs.drafts.get(&tab_id).and_then(|text| serde_json::from_str::<StoredEditor>(text).ok());
            let from_file = engine.request(&Id(collection.clone()), &Id(request_id.clone())).ok();
            let request = stored.as_ref().map(|stored| stored.request.clone()).or(from_file);
            let Some(request) = request else { continue };
            let RequestKind::Http(http) = request.kind.clone() else { continue };
            let draft = stored.as_ref().map(|stored| stored.draft.clone()).unwrap_or(http);
            let description = stored.as_ref().map(|stored| stored.description.clone()).unwrap_or_else(|| request.description.clone());
            let bodies = stored.as_ref().map(|stored| stored.bodies.clone()).unwrap_or_default();
            let tab = stored.as_ref().map(|stored| stored.tab).unwrap_or_default();
            let dirty = self.shell.tabs.tabs.iter().find(|tab| tab.id == tab_id).is_some_and(|tab| tab.dirty);
            let inherited_auth = engine.inherited_auth_kind(&Id(collection.clone()), &Id(request_id.clone())).ok().flatten();
            self.editors
                .insert(tab_id, RequestEditor { collection, request, draft, description, bodies, dirty, tab, undo_paste: None, inherited_auth });
        }
    }

    /// Load editor state for Environment and Globals tabs, and drop it with closed tabs.
    fn sync_env_editors(&mut self) {
        let Some(engine) = self.engine.clone() else { return };
        let open: Vec<(u64, bool, Option<String>)> = self
            .shell
            .tabs
            .tabs
            .iter()
            .filter_map(|tab| match &tab.doc {
                TabDoc::Environment { id } => Some((tab.id, false, Some(id.clone()))),
                TabDoc::Globals => Some((tab.id, true, None)),
                _ => None,
            })
            .collect();
        self.env_editors.retain(|id, _| open.iter().any(|(tab, _, _)| tab == id));
        for (tab_id, globals, id) in open {
            if self.env_editors.contains_key(&tab_id) {
                continue;
            }
            let environment = if globals {
                engine.globals()
            } else {
                id.as_ref()
                    .map(|id| engine.environment(&Id(id.clone())))
                    .unwrap_or_else(|| Err(postguy_engine::EngineError::NotFound { what: "environment".into(), id: String::new() }))
            };
            if let Ok(mut environment) = environment {
                if globals && environment.name.trim().is_empty() {
                    environment.name = "Globals".into();
                }
                self.env_editors
                    .insert(tab_id, EnvironmentEditorState { environment, globals, search: String::new(), show_description: true, reveal: false });
            }
        }
    }

    /// Write an autosaved environment/globals edit when the typing pause passes.
    fn flush_pending_env(&mut self, force: bool) {
        let Some((tab_id, environment, globals, at)) = &self.pending_env_save else { return };
        if !force && at.elapsed() < std::time::Duration::from_millis(400) {
            return;
        }
        let tab_id = *tab_id;
        let environment = environment.clone();
        let globals = *globals;
        self.pending_env_save = None;
        let Some(engine) = self.engine.clone() else { return };
        let result = if globals { engine.save_globals(&environment) } else { engine.save_environment(&environment) };
        match result {
            Ok(()) => {
                self.shell.tabs.set_title(tab_id, environment.name.clone());
                self.refresh();
            }
            Err(err) => self.shell.state.status = format!("Environment save failed: {err}"),
        }
    }

    fn apply_environment(&mut self, tab_id: u64, action: EnvironmentAction) {
        let Some(engine) = self.engine.clone() else { return };
        match action {
            EnvironmentAction::Save => {
                if let Some(editor) = self.env_editors.get(&tab_id) {
                    self.pending_env_save = Some((tab_id, editor.environment.clone(), editor.globals, std::time::Instant::now()));
                }
            }
            EnvironmentAction::Duplicate => {
                self.flush_pending_env(true);
                if let Some(editor) = self.env_editors.get(&tab_id)
                    && !editor.globals
                {
                    match engine.duplicate_environment(&editor.environment.id, None) {
                        Ok(copy) => {
                            self.refresh();
                            let new_tab = self.shell.tabs.open(TabDoc::Environment { id: copy.id.0.clone() }, copy.name.clone());
                            self.sync_env_editors();
                            self.shell.tabs.activate(new_tab);
                            self.shell.state.status = "Duplicated environment".into();
                        }
                        Err(err) => self.shell.state.status = err.to_string(),
                    }
                }
            }
            EnvironmentAction::Export => {
                if let Some(editor) = self.env_editors.get(&tab_id) {
                    if editor.globals {
                        self.open_export(ExportTarget::Globals);
                    } else {
                        self.open_export(ExportTarget::Environment { id: editor.environment.id.0.clone(), name: editor.environment.name.clone() });
                    }
                }
            }
            EnvironmentAction::Delete => {
                self.pending_env_save = None;
                if let Some(editor) = self.env_editors.remove(&tab_id)
                    && !editor.globals
                {
                    match engine.delete_environment(&editor.environment.id) {
                        Ok(()) => {
                            self.shell.tabs.force_close(tab_id);
                            self.shell.state.status = "Environment deleted".into();
                            self.refresh();
                        }
                        Err(err) => self.shell.state.status = err.to_string(),
                    }
                }
            }
            EnvironmentAction::CreateEnvironment => match engine.create_environment("New Environment") {
                Ok(environment) => {
                    self.refresh();
                    let tab = self.shell.tabs.open(TabDoc::Environment { id: environment.id.0.clone() }, environment.name.clone());
                    self.sync_env_editors();
                    self.shell.tabs.activate(tab);
                    self.shell.state.status = "Created environment".into();
                }
                Err(err) => self.shell.state.status = err.to_string(),
            },
            EnvironmentAction::Close => {
                self.flush_pending_env(true);
                self.env_editors.remove(&tab_id);
                self.shell.tabs.force_close(tab_id);
            }
        }
    }

    /// Load collection/folder detail state for their tabs.
    fn sync_detail_editors(&mut self) {
        let Some(engine) = self.engine.clone() else { return };
        let open: Vec<(u64, String, Option<String>)> = self
            .shell
            .tabs
            .tabs
            .iter()
            .filter_map(|tab| match &tab.doc {
                TabDoc::Collection { id } => Some((tab.id, id.clone(), None)),
                TabDoc::Folder { collection, folder } => Some((tab.id, collection.clone(), Some(folder.clone()))),
                _ => None,
            })
            .collect();
        self.detail_editors.retain(|id, _| open.iter().any(|(tab, _, _)| tab == id));
        for (tab_id, collection_id, folder_id) in open {
            if self.detail_editors.contains_key(&tab_id) {
                continue;
            }
            if let Ok(collection) = engine.collection(&Id(collection_id)) {
                self.detail_editors
                    .insert(tab_id, DetailEditorState { collection, folder: folder_id.map(Id), state: DetailState::default(), dirty: false });
            }
        }
    }

    fn flush_pending_detail(&mut self, force: bool) {
        let Some((tab_id, collection, at)) = &self.pending_detail_save else { return };
        if !force && at.elapsed() < std::time::Duration::from_millis(400) {
            return;
        }
        let tab_id = *tab_id;
        let collection = collection.clone();
        self.pending_detail_save = None;
        let Some(engine) = self.engine.clone() else { return };
        match engine.save_collection(&collection) {
            Ok(()) => {
                let name = self.detail_editors.get(&tab_id).map(|editor| detail_name(&collection, editor.folder.as_ref())).unwrap_or_default();
                self.shell.tabs.set_title(tab_id, name);
                self.refresh();
            }
            Err(err) => self.shell.state.status = format!("Save failed: {err}"),
        }
    }

    fn apply_collection(&mut self, tab_id: u64, action: CollectionAction) {
        let Some(engine) = self.engine.clone() else { return };
        match action {
            CollectionAction::Save => {
                if let Some(editor) = self.detail_editors.get_mut(&tab_id) {
                    editor.dirty = true;
                    self.pending_detail_save = Some((tab_id, editor.collection.clone(), std::time::Instant::now()));
                }
            }
            CollectionAction::Duplicate => {
                self.flush_pending_detail(true);
                if let Some(editor) = self.detail_editors.get(&tab_id) {
                    if let Some(folder) = &editor.folder {
                        match engine.duplicate_item(&editor.collection.id, folder) {
                            Ok(_) => {
                                self.refresh();
                                self.shell.state.status = "Duplicated folder".into();
                            }
                            Err(err) => self.shell.state.status = err.to_string(),
                        }
                    } else {
                        match engine.duplicate_collection(&editor.collection.id, None) {
                            Ok(copy) => {
                                self.refresh();
                                self.shell.tabs.open(TabDoc::Collection { id: copy.id.0.clone() }, copy.name.clone());
                                self.sync_detail_editors();
                                self.shell.state.status = "Duplicated collection".into();
                            }
                            Err(err) => self.shell.state.status = err.to_string(),
                        }
                    }
                }
            }
            CollectionAction::Export => {
                if let Some(editor) = self.detail_editors.get(&tab_id) {
                    self.open_export(ExportTarget::Collection { id: editor.collection.id.0.clone(), name: editor.collection.name.clone() });
                }
            }
            CollectionAction::Delete => {
                self.pending_detail_save = None;
                if let Some(editor) = self.detail_editors.remove(&tab_id) {
                    let result = if let Some(folder) = &editor.folder {
                        engine.delete_item(&editor.collection.id, folder)
                    } else {
                        engine.delete_collection(&editor.collection.id)
                    };
                    match result {
                        Ok(()) => {
                            self.shell.tabs.force_close(tab_id);
                            self.refresh();
                            self.shell.state.status = "Moved to trash".into();
                        }
                        Err(err) => self.shell.state.status = err.to_string(),
                    }
                }
            }
            CollectionAction::Run => self.shell.state.status = "The runner arrives in M3.8/M9".into(),
            CollectionAction::NewRequest => {
                if let Some(editor) = self.detail_editors.get(&tab_id) {
                    let collection_id = editor.collection.id.clone();
                    let parent = editor.folder.clone();
                    match engine.create_request(&collection_id, parent.as_ref(), "http", "New Request") {
                        Ok(request) => {
                            self.refresh();
                            self.shell.tabs.open(
                                TabDoc::Request {
                                    collection: collection_id.0.clone(),
                                    request: request.id.0.clone(),
                                    method: "GET".into(),
                                    protocol: "http".into(),
                                },
                                request.name,
                            );
                            self.sync_editors();
                            self.shell.state.status = "Created request".into();
                        }
                        Err(err) => self.shell.state.status = err.to_string(),
                    }
                }
            }
            CollectionAction::NewFolder => {
                if let Some(editor) = self.detail_editors.get(&tab_id) {
                    let parent = editor.folder.clone();
                    match engine.create_folder(&editor.collection.id, parent.as_ref(), "New Folder") {
                        Ok(folder) => {
                            self.refresh();
                            self.shell.state.status = format!("Created folder `{}`", folder.name);
                        }
                        Err(err) => self.shell.state.status = err.to_string(),
                    }
                }
            }
            CollectionAction::Close => {
                self.flush_pending_detail(true);
                self.detail_editors.remove(&tab_id);
                self.shell.tabs.force_close(tab_id);
            }
        }
    }

    fn poll_send(&mut self) {
        let Some(sending) = self.sending.take() else { return };
        let result = match sending.receiver.try_recv() {
            Ok(result) => result,
            Err(_) => {
                self.sending = Some(sending);
                return;
            }
        };
        let download = sending.download;
        let tab = sending.tab;
        match result {
            Ok((history_id, status, duration, size)) => {
                let cached = self.engine.as_ref().and_then(|engine| engine.response(Some(&history_id)));
                if let Some(cached) = cached {
                    let view = response_from_cached(&cached);
                    self.responses.insert(tab, view);
                    if download {
                        match self.save_response_body(tab) {
                            Ok(path) => self.shell.state.status = format!("{status} · {duration} ms · {size} B · saved to {path}"),
                            Err(err) => self.shell.state.status = format!("{status} · {duration} ms · {size} B · save failed: {err}"),
                        }
                    } else {
                        self.shell.state.status = format!("{status} · {duration} ms · {size} B");
                    }
                } else {
                    self.shell.state.status = format!("{status} · {duration} ms · {size} B");
                }
                self.refresh();
            }
            Err(err) => self.shell.state.status = format!("Send failed: {err}"),
        }
    }

    fn apply_builder(&mut self, tab_id: u64, action: BuilderAction) {
        let panel_tab = self.shell.tabs.tabs.iter().find(|tab| tab.id == tab_id).map(|tab| tab.title.clone()).unwrap_or_default();
        match action {
            BuilderAction::ParseCurl(text) => match postguy_interop::parse_curl(&text) {
                Ok(mut commands) => {
                    if let Some(parsed) = commands.drain(..).next()
                        && let Some(editor) = self.editors.get_mut(&tab_id)
                    {
                        editor.undo_paste = Some(editor.draft.clone());
                        editor.draft = parsed.request;
                        editor.dirty = true;
                        self.shell.tabs.set_dirty(tab_id, true);
                        self.shell.state.status = format!(
                            "Imported cURL for `{panel_tab}`{}",
                            if parsed.warnings.is_empty() { String::new() } else { format!(" ({} warnings)", parsed.warnings.len()) }
                        );
                    }
                }
                Err(err) => self.shell.state.status = format!("cURL: {err}"),
            },
            BuilderAction::UndoPaste => {
                if let Some(editor) = self.editors.get_mut(&tab_id)
                    && let Some(previous) = editor.undo_paste.take()
                {
                    editor.draft = previous;
                    editor.dirty = true;
                    self.shell.tabs.set_dirty(tab_id, true);
                    self.shell.state.status = "cURL import undone".into();
                }
            }
            BuilderAction::Save => match self.save_editor(tab_id) {
                Ok(()) => {
                    self.shell.state.status = "Saved".into();
                    self.refresh();
                }
                Err(err) => self.shell.state.status = format!("Save failed: {err}"),
            },
            BuilderAction::Send { download } => {
                self.start_send(tab_id, download);
            }
            BuilderAction::Cancel => {
                if let Some(sending) = &self.sending {
                    sending.cancel.cancel();
                    self.shell.state.status = "Cancelling…".into();
                }
            }
        }
    }

    /// Persist one builder draft through the engine (Save, and the dirty-close dialog's Save).
    fn save_editor(&mut self, tab_id: u64) -> Result<(), String> {
        let Some(editor) = self.editors.get_mut(&tab_id) else { return Ok(()) };
        editor.request.kind = RequestKind::Http(editor.draft.clone());
        editor.request.description = editor.description.clone();
        let engine = self.engine.clone().ok_or_else(|| "no workspace is open".to_string())?;
        engine.save_request(&Id(editor.collection.clone()), &editor.request).map_err(|err| err.to_string())?;
        editor.dirty = false;
        let name = editor.request.name.clone();
        self.shell.tabs.set_dirty(tab_id, false);
        self.shell.tabs.set_title(tab_id, name);
        self.shell.tabs.clear_draft(tab_id);
        Ok(())
    }

    fn save_response_body(&mut self, tab_id: u64) -> Result<String, String> {
        let Some(data) = self.responses.get(&tab_id) else { return Err("no response to save".into()) };
        let Some(engine) = &self.engine else { return Err("no workspace is open".into()) };
        let directory = engine.store_paths().files_dir();
        std::fs::create_dir_all(&directory).map_err(|err| format!("creating {}: {err}", directory.display()))?;
        let path = directory.join(format!("response-{tab_id}.bin"));
        postguy_util::atomic_write(&path, &data.body).map_err(|err| err.to_string())?;
        Ok(path.display().to_string())
    }

    fn apply_response_action(&mut self, tab_id: u64, action: ResponseAction) {
        match action {
            ResponseAction::SaveBody => match self.save_response_body(tab_id) {
                Ok(path) => self.shell.state.status = format!("Response saved to {path}"),
                Err(err) => self.shell.state.status = err,
            },
            ResponseAction::Clear => {
                self.responses.remove(&tab_id);
                self.shell.state.status = "Response cleared".into();
            }
        }
    }

    fn start_send(&mut self, tab_id: u64, download: bool) {
        let Some(editor) = self.editors.get_mut(&tab_id) else { return };
        editor.request.kind = RequestKind::Http(editor.draft.clone());
        let Some(engine) = self.engine.clone() else { return };
        let target = SendTarget::StoredDraft {
            collection: Id(editor.collection.clone()),
            request: editor.request.id.clone(),
            draft: Box::new(editor.draft.clone()),
        };
        let cancel = Cancel::new();
        let cancel_thread = cancel.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("postguy-send".into())
            .spawn(move || {
                let outcome = engine.send(target, SendOptions { environment: None, cancel: Some(cancel_thread), run_scripts: true });
                let result = outcome
                    .map(|outcome| {
                        (
                            outcome.history.id.0.clone(),
                            outcome.response.status,
                            outcome.response.timings.total_ms,
                            outcome.response.size.body_decoded_bytes as u64,
                        )
                    })
                    .map_err(|err| err.to_string());
                let _ = sender.send(result);
            })
            .ok();
        self.sending = Some(Sending { tab: tab_id, started: std::time::Instant::now(), cancel, download, receiver });
        self.shell.state.status = "Sending…".into();
    }

    fn variables_for(&self, collection: &str) -> Vec<VariableInfo> {
        let mut variables: Vec<VariableInfo> = Vec::new();
        let mut push = |scope: &str, value: &postguy_model::Variable| {
            if value.enabled {
                variables.push(VariableInfo { name: value.key.clone(), value: value.value.clone(), secret: value.secret, scope: scope.to_string() });
            }
        };
        if let Some(environment) = self
            .shell
            .sidebar_data
            .active_environment
            .as_ref()
            .and_then(|id| self.shell.sidebar_data.environments.iter().find(|environment| environment.id == *id))
        {
            for variable in &environment.variables {
                push("environment", variable);
            }
        }
        for variable in &self.shell.sidebar_data.globals.variables {
            push("globals", variable);
        }
        if let Some(value) = self.shell.sidebar_data.collections.iter().find(|value| value.id.0 == collection) {
            for variable in &value.variables {
                push("collection", variable);
            }
        }
        variables.sort_by(|a, b| a.name.cmp(&b.name));
        variables
    }
}

impl App {
    fn undo(&mut self) {
        let Some(entry) = self.undo_stack.pop() else { return };
        let result = self.apply_undo(&entry, false);
        match result {
            Ok(()) => {
                self.redo_stack.push(entry);
                self.shell.state.status = "Undone".into();
                self.refresh();
            }
            Err(err) => self.shell.state.status = err,
        }
    }

    fn redo(&mut self) {
        let Some(entry) = self.redo_stack.pop() else { return };
        let result = self.apply_undo(&entry, true);
        match result {
            Ok(()) => {
                self.undo_stack.push(entry);
                self.shell.state.status = "Redone".into();
                self.refresh();
            }
            Err(err) => self.shell.state.status = err,
        }
    }

    fn apply_undo(&self, entry: &UndoEntry, forward: bool) -> Result<(), String> {
        let Some(engine) = &self.engine else { return Err("no workspace is open".into()) };
        match entry {
            UndoEntry::Rename { target, old, new } => {
                let value = if forward { new } else { old };
                match target {
                    RenameTarget::Collection(id) => engine.rename_collection(id, value).map_err(|err| err.to_string()),
                    RenameTarget::Environment(id) => engine.rename_environment(id, value).map_err(|err| err.to_string()),
                    RenameTarget::Item { collection, id } => engine.rename_item(collection, id, value).map_err(|err| err.to_string()),
                }
            }
            UndoEntry::Move { collection, id, from_parent, from_index, to_parent, to_index } => {
                let (parent, index) = if forward { (to_parent, *to_index) } else { (from_parent, *from_index) };
                engine.move_item(collection, id, parent.as_ref(), index, false).map(|_| ()).map_err(|err| err.to_string())
            }
        }
    }

    fn apply_cookie(&mut self, action: CookieAction) {
        let Some(engine) = self.engine.clone() else { return };
        let result = match action {
            CookieAction::Delete { domain, path, name } => engine.remove_cookie(&domain, &path, &name),
            CookieAction::Clear => engine.clear_cookies(),
            CookieAction::Allow(domain) => engine.allow_cookie_domain(&domain),
            CookieAction::Disallow(domain) => engine.disallow_cookie_domain(&domain),
        };
        match result {
            Ok(()) => {
                self.refresh();
                self.shell.state.status = "Cookie jar updated".into();
            }
            Err(err) => self.shell.state.status = err.to_string(),
        }
    }

    fn apply_settings(&mut self, action: SettingsAction) {
        let Some(engine) = self.engine.clone() else { return };
        if action == SettingsAction::ResetSettings {
            self.shell.settings_value = postguy_model::Settings::default();
        }
        let settings = self.shell.settings_value.clone();
        match engine.update_settings(settings.clone()) {
            Ok(()) => {
                self.shell.state.theme_mode = match settings.theme {
                    postguy_model::Theme::System => ThemeMode::System,
                    postguy_model::Theme::Light => ThemeMode::Light,
                    postguy_model::Theme::Dark => ThemeMode::Dark,
                };
                self.shell.state.status = "Settings saved".into();
            }
            Err(err) => self.shell.state.status = format!("Settings save failed: {err}"),
        }
    }

    fn apply_trash(&mut self, action: TrashAction) {
        match action {
            TrashAction::Restore(id) => {
                if let Some(engine) = &self.engine {
                    match engine.restore_trash(&id) {
                        Ok(Some(path)) => {
                            self.shell.state.status = format!("Restored {}", path.display());
                            self.refresh();
                        }
                        Ok(None) => self.shell.state.status = "That trash entry no longer exists".into(),
                        Err(err) => self.shell.state.status = err.to_string(),
                    }
                }
            }
        }
    }

    fn apply_command(&mut self, action: CommandAction) {
        use CommandAction::*;
        match action {
            NewRequest => self.apply_tab(TabAction::NewRequest),
            NewCollection => self.apply(SidebarAction::NewCollection),
            NewEnvironment => self.apply_tab(TabAction::CreateEnvironment),
            Import => self.open_import(),
            ExportWorkspace => self.open_export(ExportTarget::Workspace),
            OpenSettings => {
                self.shell.tabs.open(TabDoc::Settings, "Settings");
            }
            OpenGlobals => {
                self.shell.tabs.open(TabDoc::Globals, "Globals");
            }
            OpenCookies => {
                self.shell.tabs.open(TabDoc::Cookies, "Cookies");
            }
            OpenTrash => {
                self.shell.tabs.open(TabDoc::Trash, "Trash");
            }
            ToggleSidebar => self.shell.state.sidebar_open = !self.shell.state.sidebar_open,
            ToggleConsole => self.shell.state.console_open = !self.shell.state.console_open,
            ToggleRightPanel => self.shell.state.right_panel_open = !self.shell.state.right_panel_open,
            ToggleTwoPane => self.shell.state.two_pane = !self.shell.state.two_pane,
            CloseTab => {
                if let Some(id) = self.shell.tabs.active {
                    self.shell.tabs.request_close(id);
                }
            }
            ReopenTab => {
                if let Some(tab) = self.shell.tabs.reopen_last() {
                    self.shell.state.status = format!("Reopened `{}`", tab.title);
                }
            }
            NextTab => self.shell.tabs.activate_next(),
            PreviousTab => self.shell.tabs.activate_previous(),
            ClearConsole => {
                if let Some(engine) = &self.engine {
                    engine.clear_console();
                }
                self.shell.state.status = "Console cleared".into();
            }
            OpenDoc(doc) => {
                let title = self.doc_title(&doc);
                self.shell.tabs.open(doc, title);
            }
        }
    }

    fn doc_title(&self, doc: &TabDoc) -> String {
        match doc {
            TabDoc::Request { collection, request, .. } => self
                .shell
                .sidebar_data
                .collections
                .iter()
                .find(|value| value.id.0 == *collection)
                .and_then(|collection| find(&collection.items, &Id(request.clone())).map(|item| item.name().to_string()))
                .unwrap_or_else(|| "Request".into()),
            TabDoc::Collection { id } => self
                .shell
                .sidebar_data
                .collections
                .iter()
                .find(|value| value.id.0 == *id)
                .map(|value| value.name.clone())
                .unwrap_or_else(|| "Collection".into()),
            TabDoc::Folder { folder, .. } => self
                .shell
                .sidebar_data
                .collections
                .iter()
                .find_map(|collection| find(&collection.items, &Id(folder.clone())).map(|item| item.name().to_string()))
                .unwrap_or_else(|| "Folder".into()),
            TabDoc::Environment { id } => self
                .shell
                .sidebar_data
                .environments
                .iter()
                .find(|value| value.id.0 == *id)
                .map(|value| value.name.clone())
                .unwrap_or_else(|| "Environment".into()),
            TabDoc::Globals => "Globals".into(),
            TabDoc::Settings => "Settings".into(),
            TabDoc::Cookies => "Cookies".into(),
            TabDoc::Trash => "Trash".into(),
            TabDoc::Runner => "Runner".into(),
            TabDoc::Welcome => "Welcome".into(),
            TabDoc::History { .. } => "History".into(),
        }
    }

    fn apply_tab(&mut self, action: TabAction) {
        let Some(engine) = self.engine.clone() else { return };
        match action {
            TabAction::SetActiveEnvironment(id) => {
                let _ = engine.set_active_environment(id.map(Id));
                self.refresh();
            }
            TabAction::CreateEnvironment => match engine.create_environment("New Environment") {
                Ok(environment) => {
                    self.refresh();
                    let tab = self.shell.tabs.open(TabDoc::Environment { id: environment.id.0.clone() }, environment.name.clone());
                    self.sync_env_editors();
                    self.shell.tabs.activate(tab);
                    self.shell.state.status = "Created environment".into();
                }
                Err(err) => self.shell.state.status = err.to_string(),
            },
            TabAction::NewRequest => {
                // A fresh workspace has no collection yet; the welcome/tab-strip New Request
                // button still has to do something useful rather than only printing a hint.
                let collections = engine.collections().unwrap_or_default();
                let collection = match collections.first() {
                    Some(collection) => collection.clone(),
                    None => match engine.create_collection("New Collection") {
                        Ok(collection) => collection,
                        Err(err) => {
                            self.shell.state.status = err.to_string();
                            return;
                        }
                    },
                };
                match engine.create_request(&collection.id, None, "http", "New Request") {
                    Ok(request) => {
                        let title = request.name.clone();
                        self.shell.tabs.open(
                            TabDoc::Request {
                                collection: collection.id.0.clone(),
                                request: request.id.0.clone(),
                                method: "GET".into(),
                                protocol: "http".into(),
                            },
                            title,
                        );
                        self.shell.state.status = "Created request".into();
                        self.refresh();
                    }
                    Err(err) => self.shell.state.status = err.to_string(),
                }
            }
            TabAction::RevealInSidebar(id) => {
                self.shell.sidebar_state.selected.clear();
                self.shell.sidebar_state.selected.insert(id.clone());
                self.shell.sidebar_state.focused = Some(id);
                self.shell.state.sidebar_open = true;
                self.shell.state.status = "Revealed in the sidebar".into();
            }
            TabAction::SaveAndClose(tab_id) => match self.save_editor(tab_id) {
                Ok(()) => {
                    self.shell.tabs.force_close(tab_id);
                    self.editors.remove(&tab_id);
                    self.shell.state.status = "Saved and closed".into();
                    self.refresh();
                }
                Err(err) => {
                    self.shell.tabs.pending_close = None;
                    self.shell.state.status = format!("Save failed: {err}; the tab stays open");
                }
            },
            TabAction::Restored(doc) => {
                self.shell.state.status = format!("Reopened {doc:?}");
            }
        }
    }
}

/// Drop tabs whose documents no longer exist in the workspace (deleted items, another machine's
/// session). Returns how many were closed.
fn validate_tabs(tabs: &mut Tabs, data: &SidebarData) -> usize {
    let collections = &data.collections;
    let environments = &data.environments;
    let history = &data.history;
    tabs.retain_valid(|doc| match doc {
        TabDoc::Collection { id } => collections.iter().any(|collection| collection.id.0 == *id),
        TabDoc::Folder { collection, folder } => {
            collections.iter().find(|value| value.id.0 == *collection).is_some_and(|value| find(&value.items, &Id(folder.clone())).is_some())
        }
        TabDoc::Request { collection, request, .. } => {
            collections.iter().find(|value| value.id.0 == *collection).is_some_and(|value| find(&value.items, &Id(request.clone())).is_some())
        }
        TabDoc::Environment { id } => environments.iter().any(|environment| environment.id.0 == *id),
        TabDoc::History { id } => history.iter().any(|entry| entry.id.0 == *id),
        _ => true,
    })
}

fn import_item(source: &str, outcome: ImportOutcome) -> ImportItemView {
    let kind = match outcome.kind {
        ImportKind::Collection => "collection",
        ImportKind::Environment => "environment",
        ImportKind::Globals => "globals",
    };
    ImportItemView { source: source.to_string(), kind: kind.to_string(), name: outcome.name, id: outcome.id, report: outcome.report, error: None }
}

fn response_from_cached(cached: &postguy_engine::CachedResponse) -> ResponseData {
    let response = &cached.response;
    let revision = cached.history_id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3));
    ResponseData {
        status: response.status,
        status_text: response.status_text.clone(),
        http_version: response.http_version.clone(),
        headers: response.headers.clone(),
        body: response.body.clone(),
        revision,
        header_bytes: response.size.header_bytes,
        body_encoded_bytes: response.size.body_encoded_bytes,
        body_decoded_bytes: response.size.body_decoded_bytes,
        content_encoding: response.size.content_encoding.map(str::to_string),
        connect_ms: response.timings.connect.total_ms,
        ttfb_ms: response.timings.ttfb_ms,
        total_ms: response.timings.total_ms,
        download_ms: response.timings.download_ms,
        redirects: cached.request.redirect_hops.len(),
        proxy: cached.request.proxy.clone(),
    }
}

fn detail_name(collection: &Collection, folder: Option<&Id>) -> String {
    let Some(folder) = folder else { return collection.name.clone() };
    fn find<'a>(items: &'a [Item], id: &Id) -> Option<&'a postguy_model::Folder> {
        let mut stack: Vec<&Item> = items.iter().collect();
        while let Some(item) = stack.pop() {
            match item {
                Item::Folder(found) => {
                    if found.id == *id {
                        return Some(found);
                    }
                    stack.extend(found.items.iter());
                }
                Item::Request(_) => {}
            }
        }
        None
    }
    find(&collection.items, folder).map(|found| found.name.clone()).unwrap_or_default()
}

fn open_engine(workspace: &std::path::Path, config: &std::path::Path) -> Result<Engine, String> {
    std::fs::create_dir_all(workspace).map_err(|err| format!("creating workspace {}: {err}", workspace.display()))?;
    std::fs::create_dir_all(config).map_err(|err| format!("creating config dir {}: {err}", config.display()))?;
    Engine::open(StorePaths::new(workspace, config)).map_err(|err| err.to_string())
}

fn locate_item(collections: &[Collection], id: &Id) -> Option<(Option<Id>, usize)> {
    fn walk(items: &[Item], id: &Id, parent: Option<&Id>) -> Option<(Option<Id>, usize)> {
        for (index, item) in items.iter().enumerate() {
            if item.id() == id {
                return Some((parent.cloned(), index));
            }
            if let Item::Folder(folder) = item
                && let Some(found) = walk(&folder.items, id, Some(&folder.id))
            {
                return Some(found);
            }
        }
        None
    }
    for collection in collections {
        if let Some(found) = walk(&collection.items, id, None) {
            return Some(found);
        }
    }
    None
}

fn item_with_collection(collections: &[Collection], id: &Id) -> Option<(Id, Item)> {
    for collection in collections {
        if let Some(item) = find(&collection.items, id) {
            return Some((collection.id.clone(), item.clone()));
        }
    }
    None
}

fn item_collection(collections: &[Collection], id: &Id) -> Option<Id> {
    item_with_collection(collections, id).map(|(collection, _)| collection)
}

fn find<'a>(items: &'a [Item], id: &Id) -> Option<&'a Item> {
    let mut stack: Vec<&Item> = items.iter().collect();
    while let Some(item) = stack.pop() {
        if item.id() == id {
            return Some(item);
        }
        if let Item::Folder(folder) = item {
            stack.extend(folder.items.iter());
        }
    }
    None
}

fn resolve_target(collections: &[Collection], target: &Id) -> Result<(Id, Option<Id>), String> {
    for collection in collections {
        if collection.id == *target {
            return Ok((collection.id.clone(), None));
        }
        if let Some(Item::Folder(folder)) = find(&collection.items, target) {
            return Ok((collection.id.clone(), Some(folder.id.clone())));
        }
    }
    Err(format!("paste target `{}` is not a collection or folder", target.0))
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_control(ui.ctx());
        self.poll_import();
        self.poll_import_picker(ui.ctx());
        self.poll_export(ui.ctx());
        let dropped: Vec<PathBuf> = ui.ctx().input(|input| input.raw.dropped_files.iter().map(|file| file.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            self.open_import();
            self.start_import_paths(dropped);
        }
        if !ui.ctx().egui_wants_keyboard_input() && ui.input(|input| input.modifiers.command && input.key_pressed(egui::Key::Z)) {
            if ui.input(|input| input.modifiers.shift) {
                self.redo();
            } else {
                self.undo();
            }
        }
        self.poll_send();
        self.sync_editors();
        self.sync_env_editors();
        self.flush_pending_env(false);
        self.sync_detail_editors();
        self.flush_pending_detail(false);
        let active = self.shell.tabs.active_tab().cloned();
        let active_id = active.as_ref().map(|tab| tab.id);
        self.shell.snippet_request = active_id.and_then(|id| self.editors.get(&id)).map(|editor| editor.draft.clone());
        let two_pane = self.shell.state.two_pane;
        let is_request = active.as_ref().is_some_and(|tab| matches!(tab.doc, TabDoc::Request { .. }));
        let panel_tab = if is_request { active_id } else { None };
        let variables = panel_tab.and_then(|id| self.editors.get(&id)).map(|editor| self.variables_for(&editor.collection)).unwrap_or_default();
        let collection_name = panel_tab
            .and_then(|id| self.editors.get(&id))
            .and_then(|editor| {
                self.shell
                    .sidebar_data
                    .collections
                    .iter()
                    .find(|collection| collection.id.0 == editor.collection)
                    .map(|collection| collection.name.clone())
            })
            .unwrap_or_default();
        let progress = self.sending.as_ref().and_then(|sending| {
            if Some(sending.tab) == panel_tab {
                Some(SendProgress { elapsed_ms: sending.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64 })
            } else {
                None
            }
        });
        let panel_result = {
            let cookie_data = std::mem::take(&mut self.shell.cookie_data);
            let mut cookie_state = std::mem::take(&mut self.shell.cookie_state);
            let mut settings_state = std::mem::take(&mut self.shell.settings);
            let mut settings_value = std::mem::take(&mut self.shell.settings_value);
            let shell = &mut self.shell;
            let editors = &mut self.editors;
            let env_editors = &mut self.env_editors;
            let detail_editors = &mut self.detail_editors;
            let trash_entries = &self.trash_entries;
            let responses = &mut self.responses;
            let viewer_states = &mut self.viewer_states;
            let mut response_action: Option<ResponseAction> = None;
            let mut panel = |ui: &mut egui::Ui, doc: &TabDoc| {
                let panel_id = active_id?;
                match doc {
                    TabDoc::Request { .. } => {
                        let editor = editors.get_mut(&panel_id)?;
                        let mut builder = RequestBuilder {
                            name: &mut editor.request.name,
                            description: &mut editor.description,
                            collection_name: &collection_name,
                            draft: &mut editor.draft,
                            bodies: &mut editor.bodies,
                            dirty: &mut editor.dirty,
                            tab: &mut editor.tab,
                            variables: &variables,
                            sending: progress,
                            undo_paste: editor.undo_paste.is_some(),
                            inherited_auth: editor.inherited_auth.as_deref(),
                        };
                        let builder_action = if let Some(data) = responses.get_mut(&panel_id) {
                            let viewer = viewer_states.entry(panel_id).or_default();
                            if two_pane {
                                let width = (ui.available_width() * 0.5).max(260.0);
                                ui.horizontal_top(|ui| {
                                    let action = ui
                                        .allocate_ui_with_layout(vec2(width, ui.available_height()), Layout::top_down(Align::Min), |ui| {
                                            builder.show(ui)
                                        })
                                        .inner;
                                    ui.separator();
                                    response_action = response_view(ui, data, viewer);
                                    action
                                })
                                .inner
                            } else {
                                let total = ui.available_height();
                                let request_height = (total * 0.52).max(170.0);
                                let action = ui
                                    .allocate_ui_with_layout(vec2(ui.available_width(), request_height), Layout::top_down(Align::Min), |ui| {
                                        builder.show(ui)
                                    })
                                    .inner;
                                ui.separator();
                                response_action = response_view(ui, data, viewer);
                                action
                            }
                        } else {
                            builder.show(ui)
                        };
                        editor.request.kind = RequestKind::Http(editor.draft.clone());
                        builder_action.map(PanelAction::Builder)
                    }
                    TabDoc::Environment { .. } | TabDoc::Globals => {
                        let editor = env_editors.get_mut(&panel_id)?;
                        let mut environment = EnvironmentEditor {
                            environment: &mut editor.environment,
                            globals: editor.globals,
                            search: &mut editor.search,
                            show_description: &mut editor.show_description,
                            reveal: &mut editor.reveal,
                        };
                        environment.show(ui).map(PanelAction::Environment)
                    }
                    TabDoc::Collection { .. } | TabDoc::Folder { .. } => {
                        let editor = detail_editors.get_mut(&panel_id)?;
                        let mut detail =
                            CollectionDetail { collection: &mut editor.collection, folder: editor.folder.clone(), state: &mut editor.state };
                        detail.show(ui).map(PanelAction::Collection)
                    }
                    TabDoc::Trash => trash_view(ui, trash_entries).map(PanelAction::Trash),
                    TabDoc::Cookies => cookie_manager(ui, &cookie_data, &mut cookie_state).map(PanelAction::Cookies),
                    TabDoc::Settings => {
                        let mut editor = postguy_ui_egui::settings::SettingsEditor { settings: &mut settings_value, state: &mut settings_state };
                        editor.show(ui).map(PanelAction::Settings)
                    }
                    _ => None,
                }
            };
            let result = shell.ui_with(ui, if active_id.is_some() { Some(&mut panel) } else { None });
            if let (Some(tab), Some(action)) = (panel_tab, response_action) {
                self.apply_response_action(tab, action);
            }
            self.shell.cookie_data = cookie_data;
            self.shell.cookie_state = cookie_state;
            self.shell.settings = settings_state;
            self.shell.settings_value = settings_value;
            result
        };
        if let Some(panel_id) = panel_tab
            && let Some(editor) = self.editors.get(&panel_id)
        {
            self.shell.tabs.set_dirty(panel_id, editor.dirty);
        }
        if let Some(action) = self.shell.action.take() {
            self.apply(action);
        }
        if let Some(action) = self.shell.tab_action.take() {
            self.apply_tab(action);
        }
        if let Some(action) = self.shell.command_action.take() {
            self.apply_command(action);
        }
        if let Some(text) = self.pending_clipboard.take() {
            ui.ctx().copy_text(text);
        }
        if let Some(action) = postguy_ui_egui::import::import_dialog(ui.ctx(), &mut self.import) {
            self.apply_import_action(action);
        }
        if let Some(request) = postguy_ui_egui::export::export_dialog(ui.ctx(), &mut self.export) {
            self.begin_export(request);
        }
        if let Some(action) = panel_result {
            match action {
                PanelAction::Builder(action) => self.apply_builder(panel_tab.unwrap_or(0), action),
                PanelAction::Environment(action) => {
                    if let Some(tab) = active_id {
                        self.apply_environment(tab, action);
                    }
                }
                PanelAction::Collection(action) => {
                    if let Some(tab) = active_id {
                        self.apply_collection(tab, action);
                    }
                }
                PanelAction::Trash(action) => self.apply_trash(action),
                PanelAction::Cookies(action) => self.apply_cookie(action),
                PanelAction::Settings(action) => self.apply_settings(action),
            }
        }
        self.save_session(false, false);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.flush_pending_env(true);
        self.flush_pending_detail(true);
        self.save_session(true, true);
    }
}

fn workspace_argument() -> PathBuf {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        if args.get(index).map(String::as_str) == Some("--workspace")
            && let Some(path) = args.get(index + 1)
        {
            return PathBuf::from(path);
        }
        index += 1;
    }
    PathBuf::from("postguy-workspace")
}

fn control_argument() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        if args.get(index).map(String::as_str) == Some("--control")
            && let Some(path) = args.get(index + 1)
        {
            return Some(PathBuf::from(path));
        }
        index += 1;
    }
    None
}

fn main() -> eframe::Result<()> {
    let workspace = workspace_argument();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]).with_min_inner_size([900.0, 560.0]).with_title("Postguy"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let mut app = App::new(workspace);
    if let Some(path) = control_argument() {
        match control::ControlServer::start(path) {
            Ok((server, _queue)) => app.attach_control(server),
            Err(error) => {
                eprintln!("control channel disabled: {error}");
                app.shell.state.status = format!("control channel disabled: {error}");
            }
        }
    }
    eframe::run_native("Postguy", options, Box::new(move |_cc| Ok(Box::new(app))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_in(dir: &std::path::Path) -> App {
        App::new(dir.join("workspace"))
    }

    fn sample_data() -> SidebarData {
        let request =
            postguy_model::Request { id: Id("r1".into()), name: "Users".into(), kind: RequestKind::Http(Default::default()), ..Default::default() };
        let collection =
            Collection { id: Id("c1".into()), name: "Payments".into(), items: vec![Item::Request(Box::new(request))], ..Default::default() };
        SidebarData {
            revision: 1,
            collections: vec![collection],
            environments: vec![postguy_model::Environment { id: Id("env1".into()), name: "Staging".into(), ..Default::default() }],
            ..SidebarData::default()
        }
    }

    #[test]
    fn tab_validation_drops_orphans_and_keeps_the_rest() {
        let mut tabs = Tabs::new();
        tabs.open(TabDoc::Collection { id: "c1".into() }, "Payments");
        tabs.open(TabDoc::Request { collection: "c1".into(), request: "r1".into(), method: "GET".into(), protocol: "http".into() }, "Users");
        tabs.open(TabDoc::Environment { id: "env1".into() }, "Staging");
        tabs.open(TabDoc::Settings, "Settings");
        assert_eq!(validate_tabs(&mut tabs, &sample_data()), 0, "everything exists");
        let mut missing_request = sample_data();
        missing_request.collections[0].items.clear();
        assert_eq!(validate_tabs(&mut tabs, &missing_request), 1);
        assert!(tabs.tabs.iter().all(|tab| !matches!(&tab.doc, TabDoc::Request { .. })));
        assert!(tabs.tabs.iter().any(|tab| matches!(tab.doc, TabDoc::Settings)));
        let mut no_environment = sample_data();
        no_environment.environments.clear();
        assert_eq!(validate_tabs(&mut tabs, &no_environment), 1, "the environment tab is dropped");
    }

    #[test]
    fn the_session_round_trips_across_an_app_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let mut app = app_in(dir.path());
            let engine = app.engine.clone().expect("engine");
            let collection = engine.create_collection("Payments").expect("collection");
            let request = engine.create_request(&collection.id, None, "http", "Users").expect("request");
            app.refresh();
            app.shell.tabs.open(
                TabDoc::Request { collection: collection.id.0.clone(), request: request.id.0.clone(), method: "GET".into(), protocol: "http".into() },
                "Users",
            );
            app.shell.tabs.open(TabDoc::Settings, "Settings");
            app.shell.sidebar_state.sections = [true, false, true];
            app.save_session(true, true);
        }
        let app = app_in(dir.path());
        assert_eq!(app.shell.tabs.tabs.len(), 2, "both tabs restored");
        assert!(app.shell.tabs.tabs.iter().any(|tab| matches!(tab.doc, TabDoc::Settings)));
        assert_eq!(app.shell.sidebar_state.sections, [true, false, true]);
        assert!(!app.shell.state.status.contains("unexpected exit"), "a clean exit is not reported as a crash");
    }

    #[test]
    fn an_unclean_exit_is_reported_on_restore() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let mut app = app_in(dir.path());
            app.shell.tabs.open(TabDoc::Settings, "Settings");
            app.save_session(false, true);
        }
        let app = app_in(dir.path());
        assert!(app.shell.state.status.contains("unexpected exit"), "{}", app.shell.state.status);
        assert_eq!(app.shell.tabs.tabs.len(), 1);
    }

    fn editor_for(app: &mut App, collection: &Id, request: &Id) -> u64 {
        app.refresh();
        let tab = app.shell.tabs.open(
            TabDoc::Request { collection: collection.0.clone(), request: request.0.clone(), method: "GET".into(), protocol: "http".into() },
            "Request",
        );
        app.sync_editors();
        tab
    }

    #[test]
    fn saving_a_builder_draft_writes_the_request_and_clears_dirty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Payments").expect("collection");
        let request = engine.create_request(&collection.id, None, "http", "Users").expect("request");
        let tab = editor_for(&mut app, &collection.id, &request.id);
        {
            let editor = app.editors.get_mut(&tab).expect("editor");
            editor.draft.url = "https://api.example.com/users?page=2".into();
            editor.draft.method = postguy_model::HttpMethod::Post;
            editor.dirty = true;
        }
        app.apply_builder(tab, BuilderAction::Save);
        let saved = engine.request(&collection.id, &request.id).expect("request");
        match saved.kind {
            RequestKind::Http(http) => {
                assert_eq!(http.url, "https://api.example.com/users?page=2");
                assert_eq!(http.method, postguy_model::HttpMethod::Post);
            }
            _ => panic!("wrong kind"),
        }
        assert!(!app.editors.get(&tab).expect("editor").dirty);
        assert!(!app.shell.tabs.tabs.iter().find(|candidate| candidate.id == tab).expect("tab").dirty);
    }

    #[test]
    fn an_unsaved_draft_survives_the_session_and_wins_over_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (collection_id, request_id) = {
            let mut app = app_in(dir.path());
            let engine = app.engine.clone().expect("engine");
            let collection = engine.create_collection("Payments").expect("collection");
            let request = engine.create_request(&collection.id, None, "http", "Users").expect("request");
            let tab = editor_for(&mut app, &collection.id, &request.id);
            {
                let editor = app.editors.get_mut(&tab).expect("editor");
                editor.draft.url = "https://draft.example.com/unsaved".into();
                editor.dirty = true;
            }
            app.shell.tabs.set_dirty(tab, true);
            app.save_session(true, true);
            (collection.id, request.id)
        };
        let mut app = app_in(dir.path());
        app.sync_editors();
        let tab = app.shell.tabs.tabs.first().expect("restored tab").id;
        let editor = app.editors.get(&tab).expect("restored editor");
        assert_eq!(editor.draft.url, "https://draft.example.com/unsaved", "the session draft wins over the saved file");
        assert!(editor.dirty);
        assert!(app.engine.as_ref().expect("engine").request(&collection_id, &request_id).is_ok());
    }

    #[test]
    fn pasting_curl_replaces_the_draft_and_marks_it_dirty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Curl").expect("collection");
        let request = engine.create_request(&collection.id, None, "http", "Paste").expect("request");
        let tab = editor_for(&mut app, &collection.id, &request.id);
        app.editors.get_mut(&tab).expect("editor").draft.url = "https://api.example.com/users".into();
        app.apply_builder(tab, BuilderAction::ParseCurl("curl -X DELETE 'https://api.example.com/items/7' -H 'X-Token: abc'".into()));
        let editor = app.editors.get(&tab).expect("editor");
        assert_eq!(editor.draft.method, postguy_model::HttpMethod::Delete);
        assert_eq!(editor.draft.url, "https://api.example.com/items/7");
        assert!(editor.dirty);
        assert!(editor.draft.headers.iter().any(|row| row.key == "X-Token" && row.value == "abc"));
        assert!(app.editors.get(&tab).expect("editor").undo_paste.is_some(), "the pre-paste draft is kept");
        app.apply_builder(tab, BuilderAction::UndoPaste);
        assert_eq!(app.editors.get(&tab).expect("editor").draft.url, "https://api.example.com/users", "undo restores the pre-paste draft");
    }

    #[test]
    fn sending_runs_off_the_ui_thread_and_reports_the_status() {
        let server = postguy_testkit::Server::start();
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Send").expect("collection");
        let request = engine.create_request(&collection.id, None, "http", "Echo").expect("request");
        let tab = editor_for(&mut app, &collection.id, &request.id);
        app.editors.get_mut(&tab).expect("editor").draft.url = format!("{}/echo", server.base_url());
        app.start_send(tab, false);
        assert!(app.sending.is_some());
        for _ in 0..200 {
            app.poll_send();
            if app.sending.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.sending.is_none(), "the send finished");
        assert!(app.shell.state.status.contains("200"), "{}", app.shell.state.status);
        assert!(!engine.history(5).expect("history").is_empty(), "the send recorded history");
        let response = app.responses.get(&tab).expect("the response is cached for the viewer");
        assert_eq!(response.status, 200);
        assert!(String::from_utf8_lossy(&response.body).contains("method"));
    }

    #[test]
    fn save_and_clear_response_actions_work_from_the_viewer() {
        let server = postguy_testkit::Server::start();
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Viewer").expect("collection");
        let request = engine.create_request(&collection.id, None, "http", "Echo").expect("request");
        let tab = editor_for(&mut app, &collection.id, &request.id);
        app.editors.get_mut(&tab).expect("editor").draft.url = format!("{}/echo", server.base_url());
        app.start_send(tab, true);
        for _ in 0..200 {
            app.poll_send();
            if app.sending.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let response = app.responses.get(&tab).expect("response");
        assert_eq!(response.status, 200);
        let saved = dir.path().join("workspace").join("files").join(format!("response-{tab}.bin"));
        assert!(saved.exists(), "Send and Download wrote the body: {}", saved.display());
        app.apply_response_action(tab, ResponseAction::SaveBody);
        assert!(app.responses.contains_key(&tab), "Save keeps the response");
        app.apply_response_action(tab, ResponseAction::Clear);
        assert!(!app.responses.contains_key(&tab), "Clear drops it");
    }

    #[test]
    fn cancelling_an_in_flight_send_reports_it() {
        let server = postguy_testkit::Server::start();
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Cancel").expect("collection");
        let request = engine.create_request(&collection.id, None, "http", "Slow").expect("request");
        let tab = editor_for(&mut app, &collection.id, &request.id);
        app.editors.get_mut(&tab).expect("editor").draft.url = format!("{}/slow?ms=3000", server.base_url());
        app.start_send(tab, false);
        std::thread::sleep(std::time::Duration::from_millis(200));
        app.apply_builder(tab, BuilderAction::Cancel);
        for _ in 0..300 {
            app.poll_send();
            if app.sending.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.sending.is_none());
        assert!(
            app.shell.state.status.contains("cancelled")
                || app.shell.state.status.contains("cancelling")
                || app.shell.state.status.contains("Cancel"),
            "{}",
            app.shell.state.status
        );
    }

    #[test]
    fn saving_from_the_dirty_close_dialog_persists_and_closes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Close").expect("collection");
        let request = engine.create_request(&collection.id, None, "http", "Users").expect("request");
        let tab = editor_for(&mut app, &collection.id, &request.id);
        {
            let editor = app.editors.get_mut(&tab).expect("editor");
            editor.draft.url = "https://saved.example.com/users".into();
            editor.dirty = true;
        }
        app.shell.tabs.set_dirty(tab, true);
        app.shell.tabs.pending_close = Some(tab);
        app.apply_tab(TabAction::SaveAndClose(tab));
        assert!(app.shell.tabs.tabs.iter().all(|candidate| candidate.id != tab), "the tab is closed after saving");
        assert_eq!(app.shell.tabs.pending_close, None);
        let saved = engine.request(&collection.id, &request.id).expect("request");
        match saved.kind {
            RequestKind::Http(http) => assert_eq!(http.url, "https://saved.example.com/users"),
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn environment_tabs_load_autosave_duplicate_and_delete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let environment = engine.create_environment("Staging").expect("environment");
        app.refresh();
        let tab = app.shell.tabs.open(TabDoc::Environment { id: environment.id.0.clone() }, "Staging");
        app.sync_env_editors();
        {
            let editor = app.env_editors.get_mut(&tab).expect("environment editor");
            editor.environment.name = "Production".into();
            editor.environment.variables.push(postguy_model::Variable {
                key: "token".into(),
                value: "hunter2".into(),
                secret: true,
                ..Default::default()
            });
        }
        app.apply_environment(tab, EnvironmentAction::Save);
        app.flush_pending_env(true);
        let saved = engine.environment(&environment.id).expect("environment");
        assert_eq!(saved.name, "Production");
        assert!(saved.variables.iter().any(|variable| variable.key == "token" && variable.value == "hunter2"));
        assert_eq!(app.shell.tabs.tabs.iter().find(|candidate| candidate.id == tab).map(|candidate| candidate.title.as_str()), Some("Production"));

        app.apply_environment(tab, EnvironmentAction::Duplicate);
        assert_eq!(engine.environments().expect("environments").len(), 2, "duplicate creates a second environment");
        let env_tabs: Vec<u64> = app.shell.tabs.tabs.iter().filter(|tab| matches!(tab.doc, TabDoc::Environment { .. })).map(|tab| tab.id).collect();
        assert!(env_tabs.len() >= 2);
        let first_tab = *env_tabs.first().expect("env tab");
        app.apply_environment(first_tab, EnvironmentAction::Delete);
        assert_eq!(engine.environments().expect("environments").len(), 1, "delete removes the file");
        assert!(app.shell.tabs.tabs.iter().all(|tab| tab.id != first_tab), "the editor tab closes");
    }

    #[test]
    fn collection_tabs_load_autosave_and_close_the_tab_after_delete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Payments").expect("collection");
        app.refresh();
        let tab = app.shell.tabs.open(TabDoc::Collection { id: collection.id.0.clone() }, "Payments");
        app.sync_detail_editors();
        {
            let editor = app.detail_editors.get_mut(&tab).expect("detail editor");
            editor.collection.name = "Billing".into();
            editor.collection.variables.push(postguy_model::Variable {
                key: "baseUrl".into(),
                value: "https://api.test".into(),
                ..Default::default()
            });
        }
        app.apply_collection(tab, CollectionAction::Save);
        app.flush_pending_detail(true);
        let saved = engine.collection(&collection.id).expect("collection");
        assert_eq!(saved.name, "Billing");
        assert!(saved.variables.iter().any(|variable| variable.key == "baseUrl"));
        assert_eq!(app.shell.tabs.tabs.iter().find(|candidate| candidate.id == tab).map(|candidate| candidate.title.as_str()), Some("Billing"));
        app.apply_collection(tab, CollectionAction::Delete);
        assert!(engine.collections().expect("collections").iter().all(|candidate| candidate.id != collection.id), "the collection file was trashed");
        assert!(app.shell.tabs.tabs.iter().all(|candidate| candidate.id != tab));
    }

    #[test]
    fn folder_detail_saves_into_its_collection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Payments").expect("collection");
        let folder = engine.create_folder(&collection.id, None, "v1").expect("folder");
        app.refresh();
        let tab = app.shell.tabs.open(TabDoc::Folder { collection: collection.id.0.clone(), folder: folder.id.0.clone() }, "v1");
        app.sync_detail_editors();
        {
            let editor = app.detail_editors.get_mut(&tab).expect("detail editor");
            editor.collection.items.iter_mut().for_each(|item| {
                if let Item::Folder(folder) = item {
                    folder.name = "v2".into();
                }
            });
        }
        app.apply_collection(tab, CollectionAction::Save);
        app.flush_pending_detail(true);
        let saved = engine.folder(&collection.id, &folder.id).expect("folder");
        assert_eq!(saved.name, "v2");
    }

    #[test]
    fn undo_and_redo_restore_renames_and_moves() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Payments").expect("collection");
        let first = engine.create_request(&collection.id, None, "http", "First").expect("first");
        let second = engine.create_request(&collection.id, None, "http", "Second").expect("second");
        app.refresh();
        app.apply(SidebarAction::Rename { id: collection.id.0.clone(), name: "Billing".into() });
        assert_eq!(engine.collection(&collection.id).expect("collection").name, "Billing");
        app.undo();
        assert_eq!(engine.collection(&collection.id).expect("collection").name, "Payments");
        app.redo();
        assert_eq!(engine.collection(&collection.id).expect("collection").name, "Billing");

        app.apply(SidebarAction::Move { id: first.id.0.clone(), parent: None, index: 1, copy: false });
        let order: Vec<String> = engine.collection(&collection.id).expect("collection").items.iter().map(|item| item.id().0.clone()).collect();
        assert_eq!(order, vec![second.id.0.clone(), first.id.0.clone()]);
        app.undo();
        let order: Vec<String> = engine.collection(&collection.id).expect("collection").items.iter().map(|item| item.id().0.clone()).collect();
        assert_eq!(order, vec![first.id.0.clone(), second.id.0.clone()], "undo restores the previous order");
        app.redo();
        let order: Vec<String> = engine.collection(&collection.id).expect("collection").items.iter().map(|item| item.id().0.clone()).collect();
        assert_eq!(order, vec![second.id.0, first.id.0], "redo re-applies the move");
    }

    #[test]
    fn import_text_runs_off_thread_lists_the_report_and_opens_the_collection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let text = r#"{"info":{"name":"Imported","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"item":[{"name":"Ping","request":{"method":"GET","url":"https://example.test/ping"}}]}"#;
        app.start_import_text(text.to_string(), false);
        for _ in 0..500 {
            app.poll_import();
            if app.import_job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!app.import.busy);
        let summary = app.import.summary.as_ref().expect("summary");
        assert_eq!(summary.items.len(), 1);
        assert_eq!(summary.items[0].name, "Imported");
        assert_eq!(summary.items[0].kind, "collection");
        app.open_imported(0);
        assert!(!app.import.open);
        assert!(app.shell.tabs.active_tab().is_some_and(|tab| matches!(tab.doc, TabDoc::Collection { .. })));
        app.apply_command(CommandAction::Import);
        assert!(app.import.open, "Cmd/Ctrl+I opens the dialog");
    }

    #[test]
    fn import_folder_reports_valid_and_invalid_files_separately() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let folder = dir.path().join("dump");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(
            folder.join("ok.json"),
            r#"{"info":{"name":"Dumped","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"item":[]}"#,
        )
        .expect("collection");
        std::fs::write(folder.join("broken.json"), "{not json").expect("broken");
        app.start_import_paths(vec![folder]);
        for _ in 0..500 {
            app.poll_import();
            if app.import_job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let summary = app.import.summary.as_ref().expect("summary");
        assert_eq!(summary.items.len(), 2);
        assert!(summary.items.iter().any(|item| item.error.is_none() && item.name == "Dumped"));
        assert!(summary.items.iter().any(|item| item.error.is_some()), "the broken file is reported, not fatal");
    }

    #[test]
    fn control_inspect_set_and_commands_drive_the_real_ui_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let initial = app.apply_control(control::ControlCommand::Inspect).expect("inspect");
        assert_eq!(initial["console"], false);
        app.apply_control(control::ControlCommand::Command("view.console".into())).expect("command");
        assert!(app.shell.state.console_open, "the registry command toggled the console");
        app.apply_control(control::ControlCommand::Set("console".into(), serde_json::json!(false))).expect("set");
        assert!(!app.shell.state.console_open);
        app.apply_control(control::ControlCommand::Set("tab".into(), serde_json::json!("settings"))).expect("tab");
        let inspected = app.apply_control(control::ControlCommand::Inspect).expect("inspect");
        assert_eq!(inspected["panel"], "settings");
        let unknown = app.apply_control(control::ControlCommand::Command("does.not.exist".into()));
        assert!(unknown.is_err(), "unknown commands are refused");
        let click = app.apply_control(control::ControlCommand::Click(10.0, 10.0));
        assert!(click.is_err(), "synthetic clicks report their unsupported status");
    }

    #[test]
    fn control_set_url_edits_the_active_request_draft() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Control").expect("collection");
        let request = engine.create_request(&collection.id, None, "http", "Echo").expect("request");
        let tab = editor_for(&mut app, &collection.id, &request.id);
        app.apply_control(control::ControlCommand::Set("url".into(), serde_json::json!("https://example.test/control"))).expect("set url");
        assert_eq!(app.editors.get(&tab).expect("editor").draft.url, "https://example.test/control");
        assert!(app.editors.get(&tab).expect("editor").dirty);
    }

    #[test]
    fn exporting_a_collection_writes_stripped_secrets_and_a_workspace_zip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let mut collection = engine.create_collection("Export me").expect("collection");
        collection.variables.push(postguy_model::Variable {
            key: "token".into(),
            value: "hunter2".into(),
            secret: true,
            ..postguy_model::Variable::default()
        });
        engine.workspace().save_collection(&collection).expect("save collection");
        let path = dir.path().join("export.postman.json");
        app.start_export(
            ExportRequest {
                target: ExportTarget::Collection { id: collection.id.0.clone(), name: collection.name.clone() },
                format: ExportFormat::Postman,
                include_secrets: false,
                default_name: "export.postman.json".into(),
                extension: "json",
            },
            path.clone(),
        );
        for _ in 0..500 {
            app.poll_export(&egui::Context::default());
            if app.export_job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(path.exists(), "the export file was written");
        let exported = std::fs::read_to_string(&path).expect("export text");
        assert!(exported.contains("token"));
        assert!(!exported.contains("hunter2"), "secret values are stripped: {exported}");

        let zip_path = dir.path().join("workspace.zip");
        app.start_export(
            ExportRequest {
                target: ExportTarget::Workspace,
                format: ExportFormat::Zip,
                include_secrets: false,
                default_name: "workspace.zip".into(),
                extension: "zip",
            },
            zip_path.clone(),
        );
        for _ in 0..500 {
            app.poll_export(&egui::Context::default());
            if app.export_job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(zip_path.exists(), "the workspace zip was written");
    }

    #[test]
    fn settings_save_persists_theme_and_proxy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        app.shell.settings_value.theme = postguy_model::Theme::Dark;
        app.shell.settings_value.proxy.mode = postguy_model::ProxyMode::Custom;
        app.shell.settings_value.proxy.host = "proxy.local".into();
        app.apply_settings(SettingsAction::Save);
        let saved = engine.settings();
        assert_eq!(saved.theme, postguy_model::Theme::Dark);
        assert_eq!(saved.proxy.mode, postguy_model::ProxyMode::Custom);
        assert_eq!(app.shell.state.theme_mode, ThemeMode::Dark);
        assert_eq!(engine.settings().proxy.host, "proxy.local");
    }

    #[test]
    fn cookie_manager_actions_hit_the_jar() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        engine.set_cookie("http://127.0.0.1:9/", "sid=abc; Path=/").expect("cookie");
        app.refresh();
        assert_eq!(app.shell.cookie_data.cookies.len(), 1);
        app.apply_cookie(CookieAction::Delete { domain: "127.0.0.1".into(), path: "/".into(), name: "sid".into() });
        assert!(engine.cookies().is_empty());
        engine.set_cookie("http://127.0.0.1:9/", "other=1; Path=/").expect("cookie");
        app.apply_cookie(CookieAction::Allow("example.com".into()));
        assert_eq!(engine.cookie_domains(), vec!["example.com".to_string()]);
        app.apply_cookie(CookieAction::Clear);
        assert!(engine.cookies().is_empty());
    }

    #[test]
    fn trash_lists_deleted_collections_and_restores_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Restore me").expect("collection");
        app.refresh();
        let tab = app.shell.tabs.open(TabDoc::Collection { id: collection.id.0.clone() }, "Restore me");
        app.sync_detail_editors();
        app.apply_collection(tab, CollectionAction::Delete);
        assert!(!app.trash_entries.is_empty(), "the delete appears in the trash");
        let entry = app.trash_entries.first().cloned().expect("trash entry");
        app.apply_trash(TrashAction::Restore(entry.id));
        assert!(engine.collections().expect("collections").iter().any(|candidate| candidate.id == collection.id), "restore puts the collection back");
    }

    #[test]
    fn globals_load_and_save_through_the_editor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let tab = app.shell.tabs.open(TabDoc::Globals, "Globals");
        app.sync_env_editors();
        {
            let editor = app.env_editors.get_mut(&tab).expect("globals editor");
            assert!(editor.globals);
            editor.environment.variables.push(postguy_model::Variable { key: "region".into(), value: "eu".into(), ..Default::default() });
        }
        app.apply_environment(tab, EnvironmentAction::Save);
        app.flush_pending_env(true);
        let globals = engine.globals().expect("globals");
        assert!(
            globals.variables.iter().any(|variable| variable.key == "region" && variable.value == "eu"),
            "status: {} vars: {:?}",
            app.shell.state.status,
            globals.variables
        );
    }

    #[test]
    fn the_selector_create_environment_action_opens_an_editor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        app.apply_tab(TabAction::CreateEnvironment);
        assert_eq!(app.engine.as_ref().expect("engine").environments().expect("environments").len(), 1);
        let tab = app.shell.tabs.active_tab().expect("environment tab");
        assert!(matches!(tab.doc, TabDoc::Environment { .. }));
        assert!(app.env_editors.contains_key(&tab.id));
    }

    #[test]
    fn the_sidebar_new_request_action_creates_a_root_request() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Payments").expect("collection");
        app.apply(SidebarAction::NewRequest { collection: collection.id.0.clone(), parent: None, protocol: RequestProtocol::Http });
        let loaded = engine.collections().expect("collections");
        assert_eq!(loaded.iter().find(|value| value.id == collection.id).map(|value| value.items.len()), Some(1));
        assert!(app.shell.tabs.active_tab().is_some_and(|tab| matches!(tab.doc, TabDoc::Request { .. })));
    }

    #[test]
    fn new_request_in_a_fresh_workspace_creates_a_collection_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        app.apply_tab(TabAction::NewRequest);
        let engine = app.engine.clone().expect("engine");
        let collections = engine.collections().expect("collections");
        assert_eq!(collections.len(), 1, "a fresh New Request creates its container");
        assert_eq!(collections[0].items.len(), 1);
        assert!(app.shell.tabs.active_tab().is_some_and(|tab| matches!(tab.doc, TabDoc::Request { .. })));
    }

    #[test]
    fn the_new_request_tab_action_creates_and_opens_a_request() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let engine = app.engine.clone().expect("engine");
        let collection = engine.create_collection("Payments").expect("collection");
        app.refresh();
        app.apply_tab(TabAction::NewRequest);
        assert_eq!(app.shell.tabs.tabs.len(), 1);
        assert!(matches!(app.shell.tabs.tabs[0].doc, TabDoc::Request { .. }));
        let loaded = engine.collections().expect("collections");
        assert_eq!(loaded.iter().find(|value| value.id == collection.id).map(|value| value.items.len()), Some(1));
    }
}
