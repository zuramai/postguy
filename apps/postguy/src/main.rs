//! Postguy desktop app (eframe + glow): owns the engine session and applies the sidebar's
//! intents (M3.2). Launch with `--workspace DIR` to choose the workspace; without it the app
//! opens `./postguy-workspace` so a fresh checkout has somewhere to write.
//!
//! The UI thread never does network I/O; M3.2 has no sends. Reads of the workspace happen when
//! an action changed it (one revision), not every frame.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use postguy_engine::{Cancel, Engine, SendOptions, SendTarget};
use postguy_model::{Collection, HttpRequest, Id, Item, Request, RequestKind};
use postguy_store::{StorePaths, config_dir};
use postguy_ui_egui::builder::{BodyDrafts, BuilderAction, BuilderTab, RequestBuilder, SendProgress};
use postguy_ui_egui::sidebar::{RequestProtocol, RowKind, SidebarAction, SidebarData};
use postguy_ui_egui::tabs::{TabAction, TabDoc, Tabs};
use postguy_ui_egui::{PostguyShell, ThemeMode};
use serde::{Deserialize, Serialize};

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
}

struct Sending {
    tab: u64,
    started: std::time::Instant,
    cancel: Cancel,
    receiver: std::sync::mpsc::Receiver<Result<(u16, u64, u64), String>>,
}

struct App {
    engine: Option<Engine>,
    shell: PostguyShell,
    clipboard: Option<Clipboard>,
    editors: BTreeMap<u64, RequestEditor>,
    sending: Option<Sending>,
    revision: u64,
    last_session: Option<String>,
    last_save: std::time::Instant,
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
                    sending: None,
                    revision: 0,
                    last_session: None,
                    last_save: std::time::Instant::now(),
                };
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
                    sending: None,
                    revision: 0,
                    last_session: None,
                    last_save: std::time::Instant::now(),
                }
            }
        }
    }

    fn refresh(&mut self) {
        let Some(engine) = &self.engine else { return };
        self.revision = self.revision.wrapping_add(1);
        let history = engine.history(200).unwrap_or_default();
        let environments = engine.environments().unwrap_or_default();
        let active = engine.active_environment().unwrap_or_default();
        let mut data = SidebarData {
            revision: self.revision,
            collections: engine.collections().unwrap_or_default(),
            environments,
            history,
            active_environment: active,
        };
        if data.environments.is_empty() {
            data.active_environment = None;
        }
        self.shell.sidebar_data = data;
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
                if collections.iter().any(|collection| collection.id == id) {
                    engine.rename_collection(&id, &name).map_err(|err| err.to_string())?;
                } else if engine.environments().map_err(|err| err.to_string())?.iter().any(|environment| environment.id == id) {
                    engine.rename_environment(&id, &name).map_err(|err| err.to_string())?;
                } else if let Some(collection) = item_collection(&collections, &id) {
                    engine.rename_item(&collection, &id, &name).map_err(|err| err.to_string())?;
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
                let parent_id = parent.as_ref().map(|id| Id(id.clone()));
                engine.move_item(&collection, &id, parent_id.as_ref(), index, copy).map_err(|err| err.to_string())?;
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
            SidebarAction::Export { .. } => Ok("Export arrives with M4".into()),
            SidebarAction::CopyAsCurl { .. } => Ok("Copy as cURL arrives with snippets (M4)".into()),
            SidebarAction::OpenInNewTab { id, .. } => Ok(format!("Open in new tab: {id} (tab model arrives in M3.3)")),
            SidebarAction::Import => Ok("Import arrives with M4".into()),
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
            self.editors.insert(tab_id, RequestEditor { collection, request, draft, description, bodies, dirty, tab });
        }
    }

    fn poll_send(&mut self) {
        let Some(sending) = &self.sending else { return };
        let Ok(result) = sending.receiver.try_recv() else { return };
        self.sending = None;
        match result {
            Ok((status, duration, size)) => {
                self.shell.state.status = format!("{status} · {duration} ms · {size} B (response viewer arrives in M3.5)");
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
            BuilderAction::Save => match self.save_editor(tab_id) {
                Ok(()) => {
                    self.shell.state.status = "Saved".into();
                    self.refresh();
                }
                Err(err) => self.shell.state.status = format!("Save failed: {err}"),
            },
            BuilderAction::Send { download } => {
                if download {
                    self.shell.state.status = "Send and Download arrives with the response viewer (M3.5)".into();
                } else {
                    self.start_send(tab_id);
                }
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

    fn start_send(&mut self, tab_id: u64) {
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
                    .map(|outcome| (outcome.response.status, outcome.response.timings.total_ms, outcome.response.size.body_decoded_bytes as u64))
                    .map_err(|err| err.to_string());
                let _ = sender.send(result);
            })
            .ok();
        self.sending = Some(Sending { tab: tab_id, started: std::time::Instant::now(), cancel, receiver });
        self.shell.state.status = "Sending…".into();
    }

    fn variables_for(&self, collection: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        if let Some(environment) = self
            .shell
            .sidebar_data
            .active_environment
            .as_ref()
            .and_then(|id| self.shell.sidebar_data.environments.iter().find(|environment| environment.id == *id))
        {
            names.extend(environment.variables.iter().filter(|variable| variable.enabled).map(|variable| variable.key.clone()));
        }
        if let Some(value) = self.shell.sidebar_data.collections.iter().find(|value| value.id.0 == collection) {
            names.extend(value.variables.iter().filter(|variable| variable.enabled).map(|variable| variable.key.clone()));
        }
        names.sort();
        names.dedup();
        names
    }
}

impl App {
    fn apply_tab(&mut self, action: TabAction) {
        let Some(engine) = self.engine.clone() else { return };
        match action {
            TabAction::SetActiveEnvironment(id) => {
                let _ = engine.set_active_environment(id.map(Id));
                self.refresh();
            }
            TabAction::NewRequest => {
                let collections = engine.collections().unwrap_or_default();
                let Some(collection) = collections.first() else {
                    self.shell.state.status = "Create a collection first".into();
                    return;
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

fn open_engine(workspace: &std::path::Path, config: &std::path::Path) -> Result<Engine, String> {
    std::fs::create_dir_all(workspace).map_err(|err| format!("creating workspace {}: {err}", workspace.display()))?;
    std::fs::create_dir_all(config).map_err(|err| format!("creating config dir {}: {err}", config.display()))?;
    Engine::open(StorePaths::new(workspace, config)).map_err(|err| err.to_string())
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
        self.poll_send();
        self.sync_editors();
        let panel_tab = self.shell.tabs.active_tab().filter(|tab| matches!(tab.doc, TabDoc::Request { .. })).map(|tab| tab.id);
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
        let builder_action = {
            let shell = &mut self.shell;
            let editors = &mut self.editors;
            let mut panel = |ui: &mut egui::Ui| {
                let panel_id = panel_tab?;
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
                };
                let action = builder.show(ui);
                editor.request.kind = RequestKind::Http(editor.draft.clone());
                action
            };
            shell.ui_with(ui, if panel_tab.is_some() { Some(&mut panel) } else { None })
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
        if let Some(action) = builder_action {
            self.apply_builder(panel_tab.unwrap_or(0), action);
        }
        self.save_session(false, false);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
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

fn main() -> eframe::Result<()> {
    let workspace = workspace_argument();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]).with_min_inner_size([900.0, 560.0]).with_title("Postguy"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let app = App::new(workspace);
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
        app.apply_builder(tab, BuilderAction::ParseCurl("curl -X DELETE 'https://api.example.com/items/7' -H 'X-Token: abc'".into()));
        let editor = app.editors.get(&tab).expect("editor");
        assert_eq!(editor.draft.method, postguy_model::HttpMethod::Delete);
        assert_eq!(editor.draft.url, "https://api.example.com/items/7");
        assert!(editor.dirty);
        assert!(editor.draft.headers.iter().any(|row| row.key == "X-Token" && row.value == "abc"));
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
        app.start_send(tab);
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
        app.start_send(tab);
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
