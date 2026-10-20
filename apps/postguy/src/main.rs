//! Postguy desktop app (eframe + glow): owns the engine session and applies the sidebar's
//! intents (M3.2). Launch with `--workspace DIR` to choose the workspace; without it the app
//! opens `./postguy-workspace` so a fresh checkout has somewhere to write.
//!
//! The UI thread never does network I/O; M3.2 has no sends. Reads of the workspace happen when
//! an action changed it (one revision), not every frame.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::PathBuf;

use postguy_engine::Engine;
use postguy_model::{Collection, Id, Item, RequestKind};
use postguy_store::{StorePaths, config_dir};
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

struct App {
    engine: Option<Engine>,
    shell: PostguyShell,
    clipboard: Option<Clipboard>,
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
                let mut app =
                    App { engine: Some(engine), shell, clipboard: None, revision: 0, last_session: None, last_save: std::time::Instant::now() };
                app.refresh();
                app.restore_session();
                app
            }
            Err(message) => {
                shell.state.status = message.clone();
                App { engine: None, shell, clipboard: None, revision: 0, last_session: None, last_save: std::time::Instant::now() }
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
            TabAction::SaveAndClose(_) => {
                self.shell.state.status = "Tab closed; the save hook lands with the request editor (M3.4)".into();
            }
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
        self.shell.ui(ui);
        if let Some(action) = self.shell.action.take() {
            self.apply(action);
        }
        if let Some(action) = self.shell.tab_action.take() {
            self.apply_tab(action);
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
