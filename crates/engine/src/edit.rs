//! Workspace edits behind the sidebar (M3.2): collections, folders, requests, examples and
//! environments. Every function loads, validates, mutates and saves through the store's atomic
//! writers; delete goes to the trash, duplicate deep-copies with fresh ids, and a move refuses
//! to drop an item into its own subtree. No function writes the collection because of a SEND.
//!
//! Traversal is iterative with a node budget, so a hostile deeply nested file cannot exhaust the
//! stack (AGENTS.md 4). Duplicate walks with an explicit depth cap.

use std::path::PathBuf;

use postguy_model::{Collection, Environment, Example, Folder, Id, Item, Request, RequestKind};
use postguy_store::Workspace;

use crate::Engine;
use crate::error::{EngineError, Result};

/// Refuse trees larger than this while walking (imports cap at 200 000 anyway).
const NODE_BUDGET: usize = 500_000;
/// Maximum duplication depth (a hostile file cannot blow the stack).
const MAX_DEPTH: usize = 256;

impl Engine {
    /// All collections in sidebar order (quarantined files skipped, matching app start).
    pub fn collections(&self) -> Result<Vec<Collection>> {
        let workspace = self.workspace();
        let mut collections = Vec::new();
        for id in workspace.collection_ids()? {
            let outcome = workspace.load_collection(&id)?;
            if outcome.quarantined_path().is_some() {
                continue;
            }
            let collection = outcome.into_value();
            if !collection.id.0.is_empty() {
                collections.push(collection);
            }
        }
        Ok(collections)
    }

    pub fn environments(&self) -> Result<Vec<Environment>> {
        let workspace = self.workspace();
        let mut environments = Vec::new();
        for id in workspace.environment_ids()? {
            let outcome = workspace.load_environment(&id)?;
            if outcome.quarantined_path().is_some() {
                continue;
            }
            let environment = outcome.into_value();
            if !environment.id.0.is_empty() {
                environments.push(environment);
            }
        }
        Ok(environments)
    }

    pub fn active_environment(&self) -> Result<Option<Id>> {
        Ok(self.workspace().active_environment_id()?)
    }

    pub fn create_collection(&self, name: &str) -> Result<Collection> {
        require_name(name)?;
        let collection = Collection { id: Id::generate(), name: name.trim().to_string(), ..Collection::default() };
        self.workspace().save_collection(&collection)?;
        Ok(collection)
    }

    pub fn rename_collection(&self, id: &Id, name: &str) -> Result<()> {
        require_name(name)?;
        let mut collection = self.load_collection(id)?;
        collection.name = name.trim().to_string();
        self.workspace().save_collection(&collection)?;
        Ok(())
    }

    pub fn duplicate_collection(&self, id: &Id, name: Option<&str>) -> Result<Collection> {
        let source = self.load_collection(id)?;
        let mut copy = source.clone();
        copy.id = Id::generate();
        copy.name = name.map(str::to_string).unwrap_or_else(|| format!("{} Copy", source.name));
        regenerate_item_ids(&mut copy.items, 0)?;
        self.workspace().save_collection(&copy)?;
        Ok(copy)
    }

    pub fn delete_collection(&self, id: &Id) -> Result<()> {
        self.workspace().delete_collection(id)?;
        Ok(())
    }

    pub fn create_environment(&self, name: &str) -> Result<Environment> {
        require_name(name)?;
        let environment = Environment { id: Id::generate(), name: name.trim().to_string(), ..Environment::default() };
        self.workspace().save_environment(&environment)?;
        Ok(environment)
    }

    pub fn rename_environment(&self, id: &Id, name: &str) -> Result<()> {
        require_name(name)?;
        let mut environment = self.load_environment(id)?;
        environment.name = name.trim().to_string();
        self.workspace().save_environment(&environment)?;
        Ok(())
    }

    pub fn duplicate_environment(&self, id: &Id, name: Option<&str>) -> Result<Environment> {
        let source = self.load_environment(id)?;
        let mut copy = source.clone();
        copy.id = Id::generate();
        copy.name = name.map(str::to_string).unwrap_or_else(|| format!("{} Copy", source.name));
        self.workspace().save_environment(&copy)?;
        Ok(copy)
    }

    pub fn delete_environment(&self, id: &Id) -> Result<()> {
        self.workspace().delete_environment(id)?;
        if self.active_environment()?.as_ref() == Some(id) {
            self.set_active_environment(None)?;
        }
        Ok(())
    }

    pub fn set_active_environment(&self, id: Option<Id>) -> Result<()> {
        self.workspace().set_active_environment(id)?;
        Ok(())
    }

    /// One environment (for its editor tab).
    pub fn environment(&self, id: &Id) -> Result<Environment> {
        self.load_environment(id)
    }

    /// Save an edited environment (autosave from the editor; secret values split by the store).
    pub fn save_environment(&self, environment: &Environment) -> Result<()> {
        require_name(&environment.name)?;
        self.workspace().save_environment(environment)?;
        Ok(())
    }

    /// The globals file (an environment by another name).
    pub fn globals(&self) -> Result<Environment> {
        Ok(self.workspace().load_globals()?.into_value())
    }

    pub fn save_globals(&self, globals: &Environment) -> Result<()> {
        // Globals have no user-visible name requirement (the file is a single globals.json).
        self.workspace().save_globals(globals)?;
        Ok(())
    }

    /// Add a request to a collection root or folder. HTTP requests start with an empty URL;
    /// gRPC/WebSocket requests with their protocol defaults.
    pub fn create_request(&self, collection: &Id, parent: Option<&Id>, protocol: &str, name: &str) -> Result<Request> {
        let mut collection_value = self.load_collection(collection)?;
        let kind = match protocol {
            "http" => RequestKind::Http(Default::default()),
            "grpc" => RequestKind::Grpc(Default::default()),
            "websocket" | "ws" => RequestKind::WebSocket(Default::default()),
            other => {
                return Err(EngineError::BadRequest { reason: format!("unknown protocol `{other}`; allowed: http, grpc, websocket") });
            }
        };
        let request = Request { id: Id::generate(), name: name.trim().to_string(), kind, ..Request::default() };
        insert_item(&mut collection_value, parent, usize::MAX, Item::Request(Box::new(request.clone())))?;
        self.workspace().save_collection(&collection_value)?;
        Ok(request)
    }

    pub fn create_folder(&self, collection: &Id, parent: Option<&Id>, name: &str) -> Result<Folder> {
        require_name(name)?;
        let mut collection_value = self.load_collection(collection)?;
        let folder = Folder { id: Id::generate(), name: name.trim().to_string(), ..Folder::default() };
        insert_item(&mut collection_value, parent, usize::MAX, Item::Folder(Box::new(folder.clone())))?;
        self.workspace().save_collection(&collection_value)?;
        Ok(folder)
    }

    pub fn create_example(&self, collection: &Id, request: &Id, name: &str) -> Result<Example> {
        let mut collection_value = self.load_collection(collection)?;
        let found =
            find_item_mut(&mut collection_value, request).ok_or_else(|| EngineError::NotFound { what: "request".into(), id: request.0.clone() })?;
        let Item::Request(request_value) = found else {
            return Err(EngineError::BadRequest { reason: format!("`{}` is a folder, not a request", request.0) });
        };
        let example =
            Example { id: Id::generate(), name: name.trim().to_string(), original_request: Some(request_value.kind.clone()), ..Example::default() };
        request_value.examples.push(example.clone());
        self.workspace().save_collection(&collection_value)?;
        Ok(example)
    }

    /// Load one request by id (for the request editor).
    pub fn request(&self, collection: &Id, request: &Id) -> Result<Request> {
        let collection_value = self.load_collection(collection)?;
        match find_item(&collection_value, request) {
            Some(Item::Request(found)) => Ok((**found).clone()),
            Some(Item::Folder(_)) => Err(EngineError::BadRequest { reason: format!("`{}` is a folder, not a request", request.0) }),
            None => Err(EngineError::NotFound { what: "request".into(), id: request.0.clone() }),
        }
    }

    /// Save an edited request (the builder's Save). The id must already exist; the name and
    /// kind-specific payload are replaced atomically through the store.
    pub fn save_request(&self, collection: &Id, request: &Request) -> Result<()> {
        require_name(&request.name)?;
        let mut collection_value = self.load_collection(collection)?;
        match find_item_mut(&mut collection_value, &request.id) {
            Some(Item::Request(existing)) => {
                existing.name = request.name.clone();
                existing.description = request.description.clone();
                existing.kind = request.kind.clone();
                existing.examples = request.examples.clone();
                existing.extras = request.extras.clone();
            }
            Some(Item::Folder(_)) => return Err(EngineError::BadRequest { reason: format!("`{}` is a folder, not a request", request.id.0) }),
            None => return Err(EngineError::NotFound { what: "request".into(), id: request.id.0.clone() }),
        }
        self.workspace().save_collection(&collection_value)?;
        Ok(())
    }

    pub fn collection(&self, id: &Id) -> Result<Collection> {
        self.load_collection(id)
    }

    pub fn rename_item(&self, collection: &Id, id: &Id, name: &str) -> Result<()> {
        require_name(name)?;
        let mut collection_value = self.load_collection(collection)?;
        if collection_value.id == *id {
            collection_value.name = name.trim().to_string();
        } else {
            match find_item_mut(&mut collection_value, id) {
                Some(Item::Request(request)) => request.name = name.trim().to_string(),
                Some(Item::Folder(folder)) => folder.name = name.trim().to_string(),
                None => {
                    // Examples are named too; search them explicitly.
                    if !rename_example(&mut collection_value, id, name.trim()) {
                        return Err(EngineError::NotFound { what: "item".into(), id: id.0.clone() });
                    }
                }
            }
        }
        self.workspace().save_collection(&collection_value)?;
        Ok(())
    }

    pub fn delete_item(&self, collection: &Id, id: &Id) -> Result<()> {
        let mut collection_value = self.load_collection(collection)?;
        if let Some(removed) = remove_item(&mut collection_value, id) {
            park_deleted(self.workspace(), collection, id, &serde_json::to_value(&removed).unwrap_or_default(), "item")?;
        } else if let Some(example) = remove_example(&mut collection_value, id) {
            park_deleted(self.workspace(), collection, id, &serde_json::to_value(&example).unwrap_or_default(), "example")?;
        } else {
            return Err(EngineError::NotFound { what: "item".into(), id: id.0.clone() });
        }
        self.workspace().save_collection(&collection_value)?;
        Ok(())
    }

    pub fn duplicate_item(&self, collection: &Id, id: &Id) -> Result<()> {
        let mut collection_value = self.load_collection(collection)?;
        if let Some(source) = find_item(&collection_value, id).cloned() {
            let mut copy = source;
            regenerate_item_ids(std::slice::from_mut(&mut copy), 0)?;
            let parent = parent_of(&collection_value, id).flatten();
            insert_item(&mut collection_value, parent.as_ref(), usize::MAX, copy)?;
        } else if let Some(example) = find_example(&collection_value, id).cloned() {
            let mut copy = example;
            copy.id = Id::generate();
            copy.name = format!("{} Copy", copy.name);
            let request_id = parent_of(&collection_value, id)
                .flatten()
                .ok_or_else(|| EngineError::NotFound { what: "request for example".into(), id: id.0.clone() })?;
            match find_item_mut(&mut collection_value, &request_id) {
                Some(Item::Request(request)) => request.examples.push(copy),
                _ => return Err(EngineError::NotFound { what: "request for example".into(), id: request_id.0 }),
            }
        } else {
            return Err(EngineError::NotFound { what: "item".into(), id: id.0.clone() });
        }
        self.workspace().save_collection(&collection_value)?;
        Ok(())
    }

    /// Move or copy an item under `parent` at `index` (`usize::MAX` = append). A move into the
    /// item's own subtree is refused; a copy keeps the original and deep-copies ids.
    pub fn move_item(&self, collection: &Id, id: &Id, parent: Option<&Id>, index: usize, copy: bool) -> Result<Item> {
        let mut collection_value = self.load_collection(collection)?;
        if let Some(parent) = parent
            && (parent == id || is_descendant(&collection_value, id, parent))
        {
            return Err(EngineError::BadRequest { reason: "an item cannot move inside itself".into() });
        }
        let mut item = remove_item(&mut collection_value, id).ok_or_else(|| EngineError::NotFound { what: "item".into(), id: id.0.clone() })?;
        if copy {
            let original = item.clone();
            insert_item(&mut collection_value, parent, index, original)?;
            regenerate_item_ids(std::slice::from_mut(&mut item), 0)?;
        }
        insert_item(&mut collection_value, parent, index, item.clone())?;
        self.workspace().save_collection(&collection_value)?;
        Ok(item)
    }

    /// Paste a clipboard item (a cut carries the original id; a copy is regenerated here).
    pub fn paste_item(&self, collection: &Id, parent: Option<&Id>, mut item: Item, fresh_ids: bool) -> Result<Item> {
        let mut collection_value = self.load_collection(collection)?;
        if fresh_ids {
            regenerate_item_ids(std::slice::from_mut(&mut item), 0)?;
        }
        make_name_unique(&collection_value, parent, &mut item);
        insert_item(&mut collection_value, parent, usize::MAX, item.clone())?;
        self.workspace().save_collection(&collection_value)?;
        Ok(item)
    }

    fn load_collection(&self, id: &Id) -> Result<Collection> {
        Ok(self.workspace().load_collection(id)?.into_value())
    }

    fn load_environment(&self, id: &Id) -> Result<Environment> {
        let environment = self.workspace().load_environment(id)?.into_value();
        if environment.id.0.is_empty() {
            return Err(EngineError::NotFound { what: "environment".into(), id: id.0.clone() });
        }
        Ok(environment)
    }
}

/// Paste keeps names unique under the target: `Name`, `Name Copy`, `Name Copy 2`, …
fn make_name_unique(collection: &Collection, parent: Option<&Id>, item: &mut Item) {
    let siblings: &[Item] = match parent {
        None => &collection.items,
        Some(parent) => match find_item(collection, parent) {
            Some(Item::Folder(folder)) => &folder.items,
            _ => &collection.items,
        },
    };
    let base = item.name().to_string();
    if !siblings.iter().any(|sibling| sibling.name() == base) {
        return;
    }
    for suffix in 1..=10_000usize {
        let candidate = if suffix == 1 { format!("{base} Copy") } else { format!("{base} Copy {suffix}") };
        if !siblings.iter().any(|sibling| sibling.name() == candidate) {
            match item {
                Item::Request(request) => request.name = candidate,
                Item::Folder(folder) => folder.name = candidate,
            }
            return;
        }
    }
}

fn require_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(EngineError::BadRequest { reason: "the name cannot be empty".into() });
    }
    Ok(())
}

/// Insert at `index` under `parent` (None = collection root; usize::MAX = append). The parent
/// must be a folder; a request or example is refused.
pub fn insert_item(collection: &mut Collection, parent: Option<&Id>, index: usize, item: Item) -> Result<()> {
    let items = match parent {
        None => &mut collection.items,
        Some(parent) => match find_item_mut(collection, parent) {
            Some(Item::Folder(folder)) => &mut folder.items,
            Some(Item::Request(_)) => {
                return Err(EngineError::BadRequest { reason: format!("`{}` is a request, not a folder", parent.0) });
            }
            None => return Err(EngineError::NotFound { what: "folder".into(), id: parent.0.clone() }),
        },
    };
    let at = index.min(items.len());
    items.insert(at, item);
    Ok(())
}

/// Find an item (request/folder/example) anywhere in the tree.
pub fn find_item<'a>(collection: &'a Collection, id: &Id) -> Option<&'a Item> {
    let mut stack: Vec<&Item> = collection.items.iter().collect();
    let mut visited = 0usize;
    while let Some(item) = stack.pop() {
        visited = visited.saturating_add(1);
        if visited > NODE_BUDGET {
            return None;
        }
        if item.id() == id {
            return Some(item);
        }
        if let Item::Folder(folder) = item {
            stack.extend(folder.items.iter());
        }
    }
    None
}

fn find_item_mut<'a>(collection: &'a mut Collection, id: &Id) -> Option<&'a mut Item> {
    // Iterative search for the path, then follow it mutably (avoids recursive borrows).
    let mut path: Vec<usize> = Vec::new();
    if !find_path(&collection.items, id, &mut path) {
        return None;
    }
    let (last, parents) = path.split_last()?;
    let mut items = &mut collection.items;
    for index in parents {
        let item = items.get_mut(*index)?;
        match item {
            Item::Folder(folder) => items = &mut folder.items,
            _ => return None,
        }
    }
    items.get_mut(*last)
}

fn find_path(items: &[Item], id: &Id, path: &mut Vec<usize>) -> bool {
    for (index, item) in items.iter().enumerate() {
        if item.id() == id {
            path.push(index);
            return true;
        }
        if let Item::Folder(folder) = item {
            path.push(index);
            if find_path(&folder.items, id, path) {
                return true;
            }
            path.pop();
        }
    }
    false
}

/// Remove an item by id; examples are removed from their request and returned as an Example.
pub fn remove_item(collection: &mut Collection, id: &Id) -> Option<Item> {
    let mut path = Vec::new();
    if !find_path(&collection.items, id, &mut path) {
        return None;
    }
    let mut items = &mut collection.items;
    for (depth, index) in path.iter().enumerate() {
        let is_last = depth + 1 == path.len();
        if is_last {
            return if *index < items.len() { Some(items.remove(*index)) } else { None };
        }
        let item = items.get_mut(*index)?;
        match item {
            Item::Folder(folder) => items = &mut folder.items,
            _ => return None,
        }
    }
    None
}

fn find_example<'a>(collection: &'a Collection, id: &Id) -> Option<&'a Example> {
    let mut stack: Vec<&Item> = collection.items.iter().collect();
    while let Some(item) = stack.pop() {
        match item {
            Item::Folder(folder) => stack.extend(folder.items.iter()),
            Item::Request(request) => {
                if let Some(example) = request.examples.iter().find(|example| example.id == *id) {
                    return Some(example);
                }
            }
        }
    }
    None
}

fn remove_example(collection: &mut Collection, id: &Id) -> Option<Example> {
    let mut stack: Vec<&mut Item> = collection.items.iter_mut().collect();
    while let Some(item) = stack.pop() {
        match item {
            Item::Folder(folder) => stack.extend(folder.items.iter_mut()),
            Item::Request(request) => {
                if let Some(index) = request.examples.iter().position(|example| example.id == *id) {
                    return Some(request.examples.remove(index));
                }
            }
        }
    }
    None
}

fn rename_example(collection: &mut Collection, id: &Id, name: &str) -> bool {
    let mut stack: Vec<&mut Item> = collection.items.iter_mut().collect();
    while let Some(item) = stack.pop() {
        match item {
            Item::Folder(folder) => stack.extend(folder.items.iter_mut()),
            Item::Request(request) => {
                if let Some(example) = request.examples.iter_mut().find(|example| example.id == *id) {
                    example.name = name.to_string();
                    return true;
                }
            }
        }
    }
    false
}

fn parent_of(collection: &Collection, id: &Id) -> Option<Option<Id>> {
    fn walk(items: &[Item], id: &Id, parent: Option<&Id>) -> Option<Option<Id>> {
        for item in items {
            if item.id() == id {
                return Some(parent.cloned());
            }
            match item {
                Item::Folder(folder) => {
                    if let Some(found) = walk(&folder.items, id, Some(&folder.id)) {
                        return Some(found);
                    }
                }
                Item::Request(request) => {
                    if request.examples.iter().any(|example| example.id == *id) {
                        return Some(Some(request.id.clone()));
                    }
                }
            }
        }
        None
    }
    walk(&collection.items, id, None)
}

fn is_descendant(collection: &Collection, ancestor: &Id, candidate: &Id) -> bool {
    let Some(item) = find_item(collection, ancestor) else { return false };
    match item {
        Item::Folder(folder) => {
            let mut stack: Vec<&Item> = folder.items.iter().collect();
            while let Some(item) = stack.pop() {
                if item.id() == candidate {
                    return true;
                }
                if let Item::Folder(folder) = item {
                    stack.extend(folder.items.iter());
                }
            }
            false
        }
        _ => false,
    }
}

fn regenerate_item_ids(items: &mut [Item], depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(EngineError::BadRequest { reason: "the tree is nested too deeply to duplicate".into() });
    }
    for item in items {
        match item {
            Item::Folder(folder) => {
                folder.id = Id::generate();
                regenerate_item_ids(&mut folder.items, depth + 1)?;
            }
            Item::Request(request) => {
                request.id = Id::generate();
                for example in &mut request.examples {
                    example.id = Id::generate();
                }
            }
        }
    }
    Ok(())
}

fn park_deleted(workspace: &Workspace, collection: &Id, id: &Id, item: &serde_json::Value, kind: &str) -> Result<()> {
    let trash = workspace.paths.trash_dir();
    std::fs::create_dir_all(&trash).map_err(|source| postguy_util::Error::Io { operation: format!("creating {}", trash.display()), source })?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|duration| duration.as_nanos()).unwrap_or(0);
    let record = serde_json::json!({
        "kind": kind,
        "collectionId": collection.0,
        "itemId": id.0,
        "deletedAt": postguy_util::time::now_unix_ms(),
        "item": item,
    });
    let bytes =
        serde_json::to_vec_pretty(&record).map_err(|source| postguy_util::Error::JsonSyntax { what: "serializing a trashed item".into(), source })?;
    let path: PathBuf = trash.join(format!("item-{nanos}.json"));
    postguy_util::atomic_write(&path, &bytes)?;
    Ok(())
}
