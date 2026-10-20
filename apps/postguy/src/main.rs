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
use postguy_store::StorePaths;
use postguy_ui_egui::sidebar::{RequestProtocol, RowKind, SidebarAction, SidebarData};
use postguy_ui_egui::{PostguyShell, ShellTab};

struct ClipboardItem {
    source_collection: Id,
    item: Item,
}

struct Clipboard {
    items: Vec<ClipboardItem>,
    cut: bool,
}

struct App {
    engine: Option<Engine>,
    shell: PostguyShell,
    clipboard: Option<Clipboard>,
    revision: u64,
}

impl App {
    fn new(workspace: PathBuf) -> App {
        let config = workspace.join(".postguy-config");
        let mut shell = PostguyShell::new(workspace.file_name().and_then(|name| name.to_str()).unwrap_or("Workspace"));
        match open_engine(&workspace, &config) {
            Ok(engine) => {
                let mut app = App { engine: Some(engine), shell, clipboard: None, revision: 0 };
                app.refresh();
                app
            }
            Err(message) => {
                shell.state.status = message.clone();
                App { engine: None, shell, clipboard: None, revision: 0 }
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
                        self.shell.state.tab = ShellTab::Detail;
                        self.shell.state.detail = Some((label, RowKind::History));
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
                let request = engine.create_request(&Id(collection), parent_id.as_ref(), protocol, "New Request").map_err(|err| err.to_string())?;
                self.shell.state.tab = ShellTab::Detail;
                self.shell.state.detail = Some((request.name.clone(), RowKind::Request));
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
