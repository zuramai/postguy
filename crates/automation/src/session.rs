//! The session behind the tool table: the open engine (which owns the workspace, runtime, jar,
//! console and history), root confinement, and the shared tree and file helpers every tool
//! calls. Sends go through the
//! engine pipeline exactly as the UI's do.

use std::path::{Path, PathBuf};

use postguy_engine::Engine;
use postguy_model::{Collection, Id, Item, Request};
use postguy_store::secrets::SecretsStore;
use postguy_store::{StorePaths, Workspace};
use serde_json::Value;

use crate::error::{AutomationError, Result};
use crate::root::Root;

/// One automation session: zero or one open workspace, inside an optional confinement root.
pub struct Session {
    root: Option<Root>,
    engine: Option<Engine>,
}

impl Session {
    pub fn new(root: Option<Root>) -> Session {
        Session { root, engine: None }
    }

    pub fn root(&self) -> Option<&Root> {
        self.root.as_ref()
    }

    /// Resolve a tool path argument. With a root, outside paths refuse identically (the check
    /// is lexical and does not touch the disk). Without one, paths pass through (the caller
    /// asked for no confinement).
    pub fn resolve(&self, given: &str) -> Result<PathBuf> {
        match &self.root {
            Some(root) => root.resolve(given),
            None => Ok(PathBuf::from(given)),
        }
    }

    /// Resolve a path that must exist (a file to import, a script to run).
    pub fn resolve_existing(&self, given: &str) -> Result<PathBuf> {
        match &self.root {
            Some(root) => root.resolve_existing(given),
            None => Ok(PathBuf::from(given)),
        }
    }

    /// Open (or create) the workspace at `given`. Creating the directory is part of the tool;
    /// a confined root still refuses outside paths before creating anything.
    pub fn open_workspace(&mut self, given: &str) -> Result<WorkspaceInfo> {
        let workspace_path = self.resolve(given)?;
        if self.root.is_some() {
            std::fs::create_dir_all(&workspace_path)
                .map_err(|err| AutomationError::Failed { message: format!("creating workspace {}: {err}", workspace_path.display()) })?;
            // Re-check after creating: a symlinked component could have redirected us.
            let root = self.root_ref()?;
            let real = std::fs::canonicalize(&workspace_path)
                .map_err(|err| AutomationError::Failed { message: format!("resolving workspace {}: {err}", workspace_path.display()) })?;
            if !real.starts_with(root.dir()) {
                return Err(AutomationError::OutsideRoot { path: given.to_string(), root: root.dir().to_string_lossy().into_owned() });
            }
        }
        // The config dir sits beside the workspace inside the same root (store takes both
        // roots; the OS config dir is a `platform` concern arriving with the shell).
        let config_dir = workspace_path.join(".postguy-config");
        std::fs::create_dir_all(&config_dir)
            .map_err(|err| AutomationError::Failed { message: format!("creating config dir {}: {err}", config_dir.display()) })?;
        let engine =
            Engine::open(StorePaths::new(workspace_path.clone(), config_dir)).map_err(|err| AutomationError::Failed { message: err.to_string() })?;
        let info = workspace_info(engine.workspace())?;
        self.engine = Some(engine);
        Ok(info)
    }

    fn root_ref(&self) -> Result<&Root> {
        self.root.as_ref().ok_or_else(|| AutomationError::Failed { message: "no root configured".to_string() })
    }

    /// The open engine (sends, console, cookies, history live here).
    pub fn engine(&self) -> Result<&Engine> {
        self.engine.as_ref().ok_or(AutomationError::NoWorkspace)
    }

    pub fn workspace(&self) -> Result<&Workspace> {
        Ok(self.engine()?.workspace())
    }

    /// Load one collection by id (NotFound is actionable).
    pub fn load_collection(&self, id: &str) -> Result<Collection> {
        let id = Id(id.to_string());
        let outcome = self.workspace()?.load_collection(&id)?;
        match outcome.into_value() {
            collection if collection.id.0.is_empty() => Err(AutomationError::NotFound { kind: "collection".into(), id: id.0 }),
            collection => Ok(collection),
        }
    }

    /// Save one collection back through the store (secret splitting included there).
    pub fn save_collection(&self, collection: &Collection) -> Result<()> {
        self.workspace()?.save_collection(collection)?;
        Ok(())
    }

    /// All collections, in order-file order. Corrupt entries are surfaced as errors here (the
    /// UI reports the quarantine; an agent gets the message).
    pub fn list_collections(&self) -> Result<Vec<Collection>> {
        let workspace = self.workspace()?;
        let mut out = Vec::new();
        for id in workspace.collection_ids()? {
            let outcome = workspace.load_collection(&id)?;
            if outcome.quarantined_path().is_some() {
                continue; // quarantined files are skipped, matching the app-start story
            }
            let collection = outcome.into_value();
            if !collection.id.0.is_empty() {
                out.push(collection);
            }
        }
        Ok(out)
    }

    pub fn environment_secrets(&self) -> Result<&SecretsStore> {
        Ok(&self.workspace()?.secrets)
    }
}

/// The `workspace_info` result shape.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub path: String,
    pub collections: usize,
    pub environments: usize,
    pub requests: usize,
    pub folders: usize,
    pub active_environment_id: Option<String>,
}

pub fn workspace_info(workspace: &Workspace) -> Result<WorkspaceInfo> {
    let mut collections = 0usize;
    let mut requests = 0usize;
    let mut folders = 0usize;
    for id in workspace.collection_ids()? {
        let outcome = workspace.load_collection(&id)?;
        if outcome.quarantined_path().is_some() {
            continue;
        }
        let collection = outcome.into_value();
        if collection.id.0.is_empty() {
            continue;
        }
        collections += 1;
        let mut stack: Vec<&Item> = collection.items.iter().collect();
        while let Some(item) = stack.pop() {
            match item {
                Item::Folder(folder) => {
                    folders += 1;
                    stack.extend(folder.items.iter());
                }
                Item::Request(_) => requests += 1,
            }
        }
    }
    Ok(WorkspaceInfo {
        path: workspace.paths.workspace().to_string_lossy().into_owned(),
        collections,
        environments: workspace.environment_ids()?.len(),
        requests,
        folders,
        active_environment_id: workspace.active_environment_id()?.map(|id| id.0),
    })
}

/// Find one item by id anywhere in the tree (depth-safe: iterative stack).
pub fn find_item<'a>(collection: &'a Collection, id: &str) -> Option<&'a Item> {
    let mut stack: Vec<&Item> = collection.items.iter().collect();
    while let Some(item) = stack.pop() {
        if item.id().0 == id {
            return Some(item);
        }
        if let Item::Folder(folder) = item {
            stack.extend(folder.items.iter());
        }
    }
    None
}

/// Find one item mutably (iterative stack of `&mut Item`).
pub fn find_item_mut<'a>(collection: &'a mut Collection, id: &str) -> Option<&'a mut Item> {
    let mut stack: Vec<&mut Item> = collection.items.iter_mut().collect();
    while let Some(item) = stack.pop() {
        if item.id().0 == id {
            return Some(item);
        }
        if let Item::Folder(folder) = item {
            stack.extend(folder.items.iter_mut());
        }
    }
    None
}

/// Remove one item from the tree, returning it (used by move/delete).
pub fn take_item(collection: &mut Collection, id: &str) -> Option<Item> {
    fn take_from(items: &mut Vec<Item>, id: &str) -> Option<Item> {
        if let Some(position) = items.iter().position(|item| item.id().0 == id) {
            return Some(items.remove(position));
        }
        for item in items.iter_mut() {
            if let Item::Folder(folder) = item
                && let Some(taken) = take_from(&mut folder.items, id)
            {
                return Some(taken);
            }
        }
        None
    }
    take_from(&mut collection.items, id)
}

/// Insert an item at the end of a folder (or the collection root when `folder_id` is None).
pub fn insert_item(collection: &mut Collection, folder_id: Option<&str>, item: Item) -> Result<()> {
    match folder_id {
        None => collection.items.push(item),
        Some(folder_id) => {
            let Some(Item::Folder(folder)) = find_item_mut(collection, folder_id) else {
                return Err(AutomationError::NotFound { kind: "folder".into(), id: folder_id.to_string() });
            };
            folder.items.push(item);
        }
    }
    Ok(())
}

/// Deep-clone an item, assigning fresh ids to it and every descendant (duplicate semantics).
pub fn clone_with_new_ids(item: &mut Item) {
    item.set_id(Id::generate());
    if let Item::Folder(folder) = item {
        folder.id = Id::generate();
        for child in folder.items.iter_mut() {
            clone_with_new_ids(child);
        }
    }
    if let Item::Request(request) = item {
        request.id = Id::generate();
        for example in request.examples.iter_mut() {
            example.id = Id::generate();
        }
    }
}

/// Build a fresh request of the requested protocol.
pub fn new_request(kind: &str, name: &str) -> Result<Request> {
    use postguy_model::RequestKind;
    let request_kind = match kind {
        "http" => RequestKind::Http(postguy_model::HttpRequest::default()),
        "grpc" => RequestKind::Grpc(postguy_model::GrpcRequest::default()),
        "websocket" | "ws" => RequestKind::WebSocket(postguy_model::WsRequest::default()),
        other => {
            return Err(AutomationError::Failed { message: format!("unknown request kind `{other}`; allowed: http, grpc, websocket") });
        }
    };
    Ok(Request {
        id: Id::generate(),
        name: name.to_string(),
        description: String::new(),
        kind: request_kind,
        examples: Vec::new(),
        extras: Default::default(),
    })
}

/// Count requests/folders inside a collection (used by `collection_list`).
pub fn counts(collection: &Collection) -> (usize, usize) {
    let mut requests = 0;
    let mut folders = 0;
    let mut stack: Vec<&Item> = collection.items.iter().collect();
    while let Some(item) = stack.pop() {
        match item {
            Item::Folder(folder) => {
                folders += 1;
                stack.extend(folder.items.iter());
            }
            Item::Request(_) => requests += 1,
        }
    }
    (folders, requests)
}

/// Read a value object's string field with a readable error for the caller.
pub fn string_arg(args: &serde_json::Map<String, Value>, tool: &str, key: &str) -> Result<String> {
    match args.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(other) => Err(AutomationError::BadArgument {
            tool: tool.to_string(),
            argument: key.to_string(),
            expected: "a JSON string".to_string(),
            got: json_kind(other).to_string(),
        }),
        None => Err(AutomationError::MissingArgument { tool: tool.to_string(), argument: key.to_string() }),
    }
}

/// Optional string argument.
pub fn optional_string(args: &serde_json::Map<String, Value>, tool: &str, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(other) => Err(AutomationError::BadArgument {
            tool: tool.to_string(),
            argument: key.to_string(),
            expected: "a JSON string".to_string(),
            got: json_kind(other).to_string(),
        }),
    }
}

/// Optional bool with a default.
pub fn bool_arg(args: &serde_json::Map<String, Value>, tool: &str, key: &str, default: bool) -> Result<bool> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(other) => Err(AutomationError::BadArgument {
            tool: tool.to_string(),
            argument: key.to_string(),
            expected: "a JSON boolean".to_string(),
            got: json_kind(other).to_string(),
        }),
    }
}

/// A short JSON type name for error messages.
pub fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Resolve `--root`-relative output paths and write atomically.
pub fn write_output(session: &Session, given: &str, bytes: &[u8]) -> Result<String> {
    let path: &Path = &session.resolve(given)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| AutomationError::Failed { message: format!("creating {}: {err}", parent.display()) })?;
    }
    postguy_util::atomic_write(path, bytes)?;
    Ok(path.to_string_lossy().into_owned())
}
