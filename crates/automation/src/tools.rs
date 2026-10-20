//! The JSON tool table (AGENTS.md 3): every user-facing capability is callable headlessly with
//! an input schema, unknown arguments rejected, and errors that name what is allowed. Tool
//! names are `snake_case`; ids are UUID text from the workspace.

use std::sync::Mutex;

use postguy_model::{Environment, Id, Item, Variable};
use postguy_util::json::{Caps, parse_json_str};
use serde_json::{Value, json};

use crate::Session;
use crate::error::{AutomationError, Result};
use crate::root::Root;
use crate::session::{
    bool_arg, clone_with_new_ids, counts, find_item, find_item_mut, insert_item, new_request, optional_string, string_arg, take_item, write_output,
};

/// One tool: name, human title, description, JSON-Schema-ish input, and safety flags.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub read_only: bool,
    pub destructive: bool,
}

/// The automation façade: a session plus the dispatch table.
pub struct Automation {
    session: Mutex<Session>,
}

impl Automation {
    /// A fresh automation; `root` confines every path argument when set.
    pub fn new(root: Option<Root>) -> Automation {
        Automation { session: Mutex::new(Session::new(root)) }
    }

    /// Resolve a path the CALLER supplies outside the tools (a `--script` file): the same
    /// confinement rules apply, including the identical outside-root refusal.
    pub fn resolve_user_path(&self, given: &str) -> Result<std::path::PathBuf> {
        let session = self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        session.resolve_existing(given)
    }

    /// Call one tool with its JSON arguments. Unknown tools and arguments are refused before
    /// any state changes; the result is the tool's JSON object.
    pub fn call(&self, tool_name: &str, args: &Value) -> Result<Value> {
        let def = tools().iter().find(|tool| tool.name == tool_name).ok_or_else(|| AutomationError::UnknownTool { tool: tool_name.to_string() })?;
        let args = match args {
            Value::Null => serde_json::Map::new(),
            Value::Object(map) => map.clone(),
            other => {
                return Err(AutomationError::BadArgument {
                    tool: tool_name.to_string(),
                    argument: "(arguments)".to_string(),
                    expected: "a JSON object".to_string(),
                    got: crate::session::json_kind(other).to_string(),
                });
            }
        };
        validate_args(def, &args)?;
        let mut session = self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        dispatch(tool_name, &args, &mut session)
    }
}

fn validate_args(def: &ToolDef, args: &serde_json::Map<String, Value>) -> Result<()> {
    let allowed: Vec<String> = def.input_schema["properties"].as_object().map(|props| props.keys().cloned().collect()).unwrap_or_default();
    for key in args.keys() {
        if !allowed.contains(key) {
            return Err(AutomationError::UnknownArgument { tool: def.name.to_string(), argument: key.clone(), allowed });
        }
    }
    if let Some(required) = def.input_schema["required"].as_array() {
        for key in required.iter().filter_map(Value::as_str) {
            if !args.contains_key(key) {
                return Err(AutomationError::MissingArgument { tool: def.name.to_string(), argument: key.to_string() });
            }
        }
    }
    Ok(())
}

macro_rules! schema {
    ($($name:literal : $spec:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut properties = serde_json::Map::new();
        #[allow(unused_mut)]
        let mut required: Vec<Value> = Vec::new();
        $(
            let (schema, is_required) = $spec;
            properties.insert($name.to_string(), schema);
            if is_required { required.push(Value::String($name.to_string())); }
        )*
        json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
    }};
}

/// A required string argument schema.
fn req(description: &'static str) -> (Value, bool) {
    (json!({"type": "string", "description": description}), true)
}

/// An optional string argument schema.
fn opt(description: &'static str) -> (Value, bool) {
    (json!({"type": "string", "description": description}), false)
}

/// An optional boolean argument schema.
fn opt_bool(description: &'static str) -> (Value, bool) {
    (json!({"type": "boolean", "description": description}), false)
}

/// The tool table. Every user-facing workflow must be reachable here before it reaches a UI.
pub fn tools() -> &'static [ToolDef] {
    &TOOLS
}

static TOOLS: std::sync::LazyLock<Vec<ToolDef>> = std::sync::LazyLock::new(|| {
    vec![
        ToolDef {
            name: "workspace_open",
            title: "Open or create a workspace",
            description: "Create the folder if needed and open it; takes the workspace lock. Paths stay inside --root when one is set.",
            input_schema: schema! { "path": req("workspace folder") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "workspace_info",
            title: "Workspace summary",
            description: "Path, counts and the active environment id of the open workspace.",
            input_schema: schema! {},
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "collection_list",
            title: "List collections",
            description: "Collections in sidebar order with id, name and counts.",
            input_schema: schema! {},
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "collection_get",
            title: "Get one collection",
            description: "The full collection JSON.",
            input_schema: schema! { "collection": req("collection id") },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "collection_create",
            title: "Create a collection",
            description: "A new empty collection with a generated id.",
            input_schema: schema! { "name": req("collection name") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "collection_rename",
            title: "Rename a collection",
            description: "New name; empty names are refused.",
            input_schema: schema! { "collection": req("collection id"), "name": req("new name") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "collection_duplicate",
            title: "Duplicate a collection",
            description: "Deep copy with fresh ids for every item.",
            input_schema: schema! { "collection": req("collection id"), "name": opt("name for the copy (default: `<name> Copy`)") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "collection_delete",
            title: "Delete a collection",
            description: "Moves the collection file to the workspace trash and drops its secrets.",
            input_schema: schema! { "collection": req("collection id") },
            read_only: false,
            destructive: true,
        },
        ToolDef {
            name: "folder_create",
            title: "Create a folder",
            description: "Inside the collection root or another folder.",
            input_schema: schema! {
                "collection": req("collection id"),
                "parent": opt("parent folder id (omit for the collection root)"),
                "name": req("folder name")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "folder_rename",
            title: "Rename a folder",
            description: "New name; empty names are refused.",
            input_schema: schema! { "collection": req("collection id"), "folder": req("folder id"), "name": req("new name") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "folder_delete",
            title: "Delete a folder",
            description: "Deletes the folder and its contents; the subtree JSON is parked under .trash/item-<time>.json.",
            input_schema: schema! { "collection": req("collection id"), "folder": req("folder id") },
            read_only: false,
            destructive: true,
        },
        ToolDef {
            name: "request_create",
            title: "Create a request",
            description: "HTTP, gRPC or WebSocket; the protocol is fixed for the request's life.",
            input_schema: schema! {
                "collection": req("collection id"),
                "folder": opt("folder id (omit for the collection root)"),
                "kind": req("http | grpc | websocket"),
                "name": opt("request name (default `New Request`)")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "request_get",
            title: "Get one request",
            description: "The request JSON including its kind-specific body.",
            input_schema: schema! { "collection": req("collection id"), "request": req("request id") },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "request_update",
            title: "Update a request",
            description: "Replaces name, description and/or the kind-specific `definition` (the serialized RequestKind object).",
            input_schema: schema! {
                "collection": req("collection id"),
                "request": req("request id"),
                "name": opt("new name"),
                "description": opt("new description"),
                "definition": (json!({"type": "object", "description": "serialized RequestKind ({\"type\":\"http\",...})"}), false),
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "request_move",
            title: "Move a request",
            description: "Into a folder, or the collection root when `folder` is omitted.",
            input_schema: schema! {
                "collection": req("collection id"),
                "request": req("request id"),
                "folder": opt("target folder id (omit for the collection root)")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "request_duplicate",
            title: "Duplicate a request",
            description: "Deep copy into the same parent with fresh ids.",
            input_schema: schema! {
                "collection": req("collection id"),
                "request": req("request id"),
                "name": opt("name for the copy (default: `<name> Copy`)")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "request_delete",
            title: "Delete a request",
            description: "Removes it from the collection; the JSON is parked under .trash/item-<time>.json.",
            input_schema: schema! { "collection": req("collection id"), "request": req("request id") },
            read_only: false,
            destructive: true,
        },
        ToolDef {
            name: "env_list",
            title: "List environments",
            description: "Id and name in order-file order, plus which one is active.",
            input_schema: schema! {},
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "env_create",
            title: "Create an environment",
            description: "An empty environment with a generated id.",
            input_schema: schema! { "name": req("environment name") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "env_rename",
            title: "Rename an environment",
            description: "New name; empty names are refused.",
            input_schema: schema! { "environment": req("environment id"), "name": req("new name") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "env_duplicate",
            title: "Duplicate an environment",
            description: "Deep copy with a fresh id; secret values copy too.",
            input_schema: schema! { "environment": req("environment id"), "name": opt("name for the copy") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "env_delete",
            title: "Delete an environment",
            description: "Trashes the file, clears its secrets, and clears the active id when it pointed here.",
            input_schema: schema! { "environment": req("environment id") },
            read_only: false,
            destructive: true,
        },
        ToolDef {
            name: "env_set_active",
            title: "Set the active environment",
            description: "`environment: null` clears it.",
            input_schema: schema! { "environment": opt("environment id or null") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "env_get",
            title: "Get one environment",
            description: "The environment JSON with secret values rehydrated for the caller.",
            input_schema: schema! { "environment": req("environment id") },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "env_set_var",
            title: "Set an environment variable",
            description: "Creates the row or updates it; `secret: true` stores the value outside the environment file.",
            input_schema: schema! {
                "environment": req("environment id"),
                "key": req("variable name"),
                "value": req("value (empty clears it)"),
                "secret": opt_bool("store the value in the secrets file"),
                "enabled": opt_bool("row participates (default true)"),
                "description": opt("row description")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "env_unset_var",
            title: "Unset an environment variable",
            description: "Removes the row and its stored secret.",
            input_schema: schema! { "environment": req("environment id"), "key": req("variable name") },
            read_only: false,
            destructive: true,
        },
        ToolDef {
            name: "globals_get",
            title: "Get globals",
            description: "The globals file with secret values rehydrated.",
            input_schema: schema! {},
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "globals_set_var",
            title: "Set a global variable",
            description: "Creates or updates one globals row (secret values split out).",
            input_schema: schema! {
                "key": req("variable name"),
                "value": req("value (empty clears it)"),
                "secret": opt_bool("store the value in the secrets file"),
                "enabled": opt_bool("row participates (default true)"),
                "description": opt("row description")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "import_text",
            title: "Import Postman JSON text",
            description: "Collection (v2.0/v2.1), environment or globals; returns the ImportReport.",
            input_schema: schema! {
                "text": req("the JSON document"),
                "format": opt("\"auto\" (default) — other formats arrive with M4")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "import_file",
            title: "Import a Postman JSON file",
            description: "Size-capped read, then the same mapping as import_text; the path resolves inside --root.",
            input_schema: schema! { "path": req("file path") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "export_collection",
            title: "Export a collection",
            description: "Postman v2.1 JSON or the Postguy native file; writes `out` or returns the JSON.",
            input_schema: schema! {
                "collection": req("collection id"),
                "format": opt("\"postman\" (default) or \"native\""),
                "out": opt("file path to write (inside --root)")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "export_environment",
            title: "Export an environment",
            description: "Postman environment shape; writes `out` or returns the JSON.",
            input_schema: schema! { "environment": req("environment id"), "out": opt("file path to write") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "export_globals",
            title: "Export globals",
            description: "Postman globals shape; writes `out` or returns the JSON.",
            input_schema: schema! { "out": opt("file path to write") },
            read_only: false,
            destructive: false,
        },
    ]
});

/// Run one validated tool call.
fn dispatch(tool: &str, args: &serde_json::Map<String, Value>, session: &mut Session) -> Result<Value> {
    match tool {
        "workspace_open" => {
            let path = string_arg(args, tool, "path")?;
            let info = session.open_workspace(&path)?;
            Ok(serde_json::to_value(info).map_err(json_error)?)
        }
        "workspace_info" => {
            let workspace = session.workspace()?;
            Ok(serde_json::to_value(crate::session::workspace_info(workspace)?).map_err(json_error)?)
        }
        "collection_list" => {
            let workspace = session.workspace()?;
            let mut list = Vec::new();
            for id in workspace.collection_ids()? {
                let outcome = workspace.load_collection(&id)?;
                if outcome.quarantined_path().is_some() {
                    continue;
                }
                let collection = outcome.into_value();
                if collection.id.0.is_empty() {
                    continue;
                }
                let (folders, requests) = counts(&collection);
                list.push(json!({"id": collection.id.0, "name": collection.name, "folders": folders, "requests": requests}));
            }
            Ok(json!({"collections": list}))
        }
        "collection_get" => {
            let collection = session.load_collection(&string_arg(args, tool, "collection")?)?;
            Ok(json!({"collection": collection}))
        }
        "collection_create" => {
            let name = string_arg(args, tool, "name")?;
            require_non_empty(tool, "name", &name)?;
            let collection = postguy_model::Collection { id: Id::generate(), name, ..Default::default() };
            session.save_collection(&collection)?;
            Ok(json!({"collection": collection.id.0}))
        }
        "collection_rename" => {
            let id = string_arg(args, tool, "collection")?;
            let name = string_arg(args, tool, "name")?;
            require_non_empty(tool, "name", &name)?;
            let mut collection = session.load_collection(&id)?;
            collection.name = name;
            session.save_collection(&collection)?;
            Ok(json!({"collection": id, "name": collection.name}))
        }
        "collection_duplicate" => {
            let id = string_arg(args, tool, "collection")?;
            let mut collection = session.load_collection(&id)?;
            collection.id = Id::generate();
            let new_name = optional_string(args, tool, "name")?.unwrap_or_else(|| format!("{} Copy", collection.name));
            collection.name = new_name;
            for item in collection.items.iter_mut() {
                clone_with_new_ids(item);
            }
            session.save_collection(&collection)?;
            Ok(json!({"collection": collection.id.0, "name": collection.name}))
        }
        "collection_delete" => {
            let id = string_arg(args, tool, "collection")?;
            session.workspace()?.delete_collection(&Id(id.clone()))?;
            Ok(json!({"deleted": id}))
        }
        "folder_create" => {
            let collection_id = string_arg(args, tool, "collection")?;
            let parent = optional_string(args, tool, "parent")?;
            let name = string_arg(args, tool, "name")?;
            require_non_empty(tool, "name", &name)?;
            let mut collection = session.load_collection(&collection_id)?;
            let folder = postguy_model::Folder { id: Id::generate(), name, ..Default::default() };
            insert_item(&mut collection, parent.as_deref(), Item::Folder(Box::new(folder.clone())))?;
            session.save_collection(&collection)?;
            Ok(json!({"folder": folder.id.0}))
        }
        "folder_rename" => {
            let collection_id = string_arg(args, tool, "collection")?;
            let folder_id = string_arg(args, tool, "folder")?;
            let name = string_arg(args, tool, "name")?;
            require_non_empty(tool, "name", &name)?;
            let mut collection = session.load_collection(&collection_id)?;
            match find_item_mut(&mut collection, &folder_id) {
                Some(Item::Folder(folder)) => folder.name = name,
                Some(Item::Request(_)) => {
                    return Err(AutomationError::Failed { message: format!("`{folder_id}` is a request, not a folder") });
                }
                None => return Err(AutomationError::NotFound { kind: "folder".into(), id: folder_id }),
            }
            session.save_collection(&collection)?;
            Ok(json!({"folder": folder_id}))
        }
        "folder_delete" => {
            let collection_id = string_arg(args, tool, "collection")?;
            let folder_id = string_arg(args, tool, "folder")?;
            let mut collection = session.load_collection(&collection_id)?;
            match take_item(&mut collection, &folder_id) {
                Some(item @ Item::Folder(_)) => {
                    park_deleted(session, &collection_id, &folder_id, &item)?;
                    session.save_collection(&collection)?;
                    Ok(json!({"deleted": folder_id}))
                }
                Some(Item::Request(_)) => Err(AutomationError::Failed { message: format!("`{folder_id}` is a request; use request_delete") }),
                None => Err(AutomationError::NotFound { kind: "folder".into(), id: folder_id }),
            }
        }
        "request_create" => {
            let collection_id = string_arg(args, tool, "collection")?;
            let folder = optional_string(args, tool, "folder")?;
            let kind = string_arg(args, tool, "kind")?;
            let name = optional_string(args, tool, "name")?.unwrap_or_else(|| "New Request".to_string());
            let mut collection = session.load_collection(&collection_id)?;
            let request = new_request(&kind, &name)?;
            let request_id = request.id.0.clone();
            insert_item(&mut collection, folder.as_deref(), Item::Request(Box::new(request)))?;
            session.save_collection(&collection)?;
            Ok(json!({"request": request_id}))
        }
        "request_get" => {
            let collection = session.load_collection(&string_arg(args, tool, "collection")?)?;
            let request_id = string_arg(args, tool, "request")?;
            match find_item(&collection, &request_id) {
                Some(Item::Request(request)) => Ok(json!({"request": request})),
                Some(Item::Folder(_)) => Err(AutomationError::Failed { message: format!("`{request_id}` is a folder, not a request") }),
                None => Err(AutomationError::NotFound { kind: "request".into(), id: request_id }),
            }
        }
        "request_update" => {
            let collection_id = string_arg(args, tool, "collection")?;
            let request_id = string_arg(args, tool, "request")?;
            let name = optional_string(args, tool, "name")?;
            let description = optional_string(args, tool, "description")?;
            let definition = args.get("definition").cloned();
            let mut collection = session.load_collection(&collection_id)?;
            match find_item_mut(&mut collection, &request_id) {
                Some(Item::Request(request)) => {
                    if let Some(name) = name {
                        require_non_empty(tool, "name", &name)?;
                        request.name = name;
                    }
                    if let Some(description) = description {
                        request.description = description;
                    }
                    if let Some(definition) = definition {
                        request.kind = serde_json::from_value(definition).map_err(|err| AutomationError::BadArgument {
                            tool: tool.to_string(),
                            argument: "definition".to_string(),
                            expected: format!("a serialized RequestKind ({err})"),
                            got: "unparseable object".to_string(),
                        })?;
                    }
                }
                Some(Item::Folder(_)) => {
                    return Err(AutomationError::Failed { message: format!("`{request_id}` is a folder, not a request") });
                }
                None => return Err(AutomationError::NotFound { kind: "request".into(), id: request_id }),
            }
            session.save_collection(&collection)?;
            Ok(json!({"request": request_id}))
        }
        "request_move" => {
            let collection_id = string_arg(args, tool, "collection")?;
            let request_id = string_arg(args, tool, "request")?;
            let folder = optional_string(args, tool, "folder")?;
            let mut collection = session.load_collection(&collection_id)?;
            let Some(item) = take_item(&mut collection, &request_id) else {
                return Err(AutomationError::NotFound { kind: "request".into(), id: request_id });
            };
            if !matches!(item, Item::Request(_)) {
                return Err(AutomationError::Failed { message: format!("`{request_id}` is a folder; use folder ops") });
            }
            insert_item(&mut collection, folder.as_deref(), item)?;
            session.save_collection(&collection)?;
            Ok(json!({"request": request_id, "folder": folder}))
        }
        "request_duplicate" => {
            let collection_id = string_arg(args, tool, "collection")?;
            let request_id = string_arg(args, tool, "request")?;
            let name = optional_string(args, tool, "name")?;
            let mut collection = session.load_collection(&collection_id)?;
            let Some(Item::Request(original)) = find_item(&collection, &request_id) else {
                return Err(AutomationError::NotFound { kind: "request".into(), id: request_id });
            };
            let mut copy = (**original).clone();
            clone_with_new_ids(&mut Item::Request(Box::new(copy.clone())));
            copy.id = Id::generate();
            copy.name = name.unwrap_or_else(|| format!("{} Copy", original.name));
            let parent = parent_of(&collection, &request_id);
            let copy_id = copy.id.0.clone();
            insert_item(&mut collection, parent.as_deref(), Item::Request(Box::new(copy)))?;
            session.save_collection(&collection)?;
            Ok(json!({"request": copy_id}))
        }
        "request_delete" => {
            let collection_id = string_arg(args, tool, "collection")?;
            let request_id = string_arg(args, tool, "request")?;
            let mut collection = session.load_collection(&collection_id)?;
            match take_item(&mut collection, &request_id) {
                Some(item @ Item::Request(_)) => {
                    park_deleted(session, &collection_id, &request_id, &item)?;
                    session.save_collection(&collection)?;
                    Ok(json!({"deleted": request_id}))
                }
                Some(Item::Folder(_)) => Err(AutomationError::Failed { message: format!("`{request_id}` is a folder; use folder_delete") }),
                None => Err(AutomationError::NotFound { kind: "request".into(), id: request_id }),
            }
        }
        "env_list" => {
            let workspace = session.workspace()?;
            let active = workspace.active_environment_id()?;
            let mut list = Vec::new();
            for id in workspace.environment_ids()? {
                let outcome = workspace.load_environment(&id)?;
                if outcome.quarantined_path().is_some() {
                    continue;
                }
                let environment = outcome.into_value();
                list.push(json!({
                    "id": id.0,
                    "name": environment.name,
                    "active": active.as_ref() == Some(&id)
                }));
            }
            Ok(json!({"environments": list}))
        }
        "env_create" => {
            let name = string_arg(args, tool, "name")?;
            require_non_empty(tool, "name", &name)?;
            let environment = Environment { id: Id::generate(), name, ..Default::default() };
            session.workspace()?.save_environment(&environment)?;
            Ok(json!({"environment": environment.id.0}))
        }
        "env_rename" => {
            let id = string_arg(args, tool, "environment")?;
            let name = string_arg(args, tool, "name")?;
            require_non_empty(tool, "name", &name)?;
            let mut environment = load_environment(session, &id)?;
            environment.name = name;
            session.workspace()?.save_environment(&environment)?;
            Ok(json!({"environment": id, "name": environment.name}))
        }
        "env_duplicate" => {
            let id = string_arg(args, tool, "environment")?;
            let mut environment = load_environment(session, &id)?;
            environment.id = Id::generate();
            environment.name = optional_string(args, tool, "name")?.unwrap_or_else(|| format!("{} Copy", environment.name));
            for variable in environment.variables.iter_mut() {
                if variable.secret && !variable.value.is_empty() {
                    // The duplicate's secret values move into ITS owner id on save.
                }
            }
            session.workspace()?.save_environment(&environment)?;
            Ok(json!({"environment": environment.id.0, "name": environment.name}))
        }
        "env_delete" => {
            let id = string_arg(args, tool, "environment")?;
            session.workspace()?.delete_environment(&Id(id.clone()))?;
            Ok(json!({"deleted": id}))
        }
        "env_set_active" => {
            let id = optional_string(args, tool, "environment")?;
            session.workspace()?.set_active_environment(id.clone().map(Id))?;
            Ok(json!({"active": id}))
        }
        "env_get" => {
            let id = string_arg(args, tool, "environment")?;
            let environment = load_environment(session, &id)?;
            Ok(json!({"environment": environment}))
        }
        "env_set_var" => {
            let id = string_arg(args, tool, "environment")?;
            let mut environment = load_environment(session, &id)?;
            upsert_variable(&mut environment.variables, args, tool)?;
            session.workspace()?.save_environment(&environment)?;
            Ok(json!({"environment": id}))
        }
        "env_unset_var" => {
            let id = string_arg(args, tool, "environment")?;
            let key = string_arg(args, tool, "key")?;
            let mut environment = load_environment(session, &id)?;
            environment.variables.retain(|variable| variable.key != key);
            session.workspace()?.save_environment(&environment)?;
            Ok(json!({"environment": id, "key": key}))
        }
        "globals_get" => {
            let workspace = session.workspace()?;
            let globals = workspace.load_globals()?.into_value();
            Ok(json!({"globals": globals}))
        }
        "globals_set_var" => {
            let workspace = session.workspace()?;
            let mut globals = workspace.load_globals()?.into_value();
            if globals.id.0.is_empty() {
                globals.id = Id(postguy_store::workspace_file::GLOBALS_OWNER.to_string());
                globals.name = "Globals".to_string();
            }
            upsert_variable(&mut globals.variables, args, tool)?;
            workspace.save_globals(&globals)?;
            Ok(json!({"globals": true}))
        }
        "import_text" => {
            let text = string_arg(args, tool, "text")?;
            let format = optional_string(args, tool, "format")?.unwrap_or_else(|| "auto".to_string());
            import_text(session, &text, &format)
        }
        "import_file" => {
            let path = string_arg(args, tool, "path")?;
            let resolved = session.resolve_existing(&path)?;
            let bytes = postguy_util::read_file_capped(&resolved, 100 << 20)?;
            let text = String::from_utf8(bytes).map_err(|_| AutomationError::Failed {
                message: format!("{} is not UTF-8; only JSON text imports are supported in M1", resolved.display()),
            })?;
            import_text(session, &text, "auto")
        }
        "export_collection" => {
            let id = string_arg(args, tool, "collection")?;
            let collection = session.load_collection(&id)?;
            let format = optional_string(args, tool, "format")?.unwrap_or_else(|| "postman".to_string());
            let value = match format.as_str() {
                "postman" => postguy_interop::export_collection(&collection),
                "native" => serde_json::to_value(&collection).map_err(json_error)?,
                other => {
                    return Err(AutomationError::BadArgument {
                        tool: tool.to_string(),
                        argument: "format".to_string(),
                        expected: "\"postman\" or \"native\"".to_string(),
                        got: other.to_string(),
                    });
                }
            };
            finish_export(session, args, value)
        }
        "export_environment" => {
            let id = string_arg(args, tool, "environment")?;
            let environment = load_environment(session, &id)?;
            let value = postguy_interop::export_environment(&environment, "environment");
            finish_export(session, args, value)
        }
        "export_globals" => {
            let workspace = session.workspace()?;
            let globals = workspace.load_globals()?.into_value();
            let value = postguy_interop::export_environment(&globals, "globals");
            finish_export(session, args, value)
        }
        other => Err(AutomationError::UnknownTool { tool: other.to_string() }),
    }
}

fn load_environment(session: &Session, id: &str) -> Result<Environment> {
    let outcome = session.workspace()?.load_environment(&Id(id.to_string()))?;
    let environment = outcome.into_value();
    if environment.id.0.is_empty() {
        return Err(AutomationError::NotFound { kind: "environment".into(), id: id.to_string() });
    }
    Ok(environment)
}

fn upsert_variable(variables: &mut Vec<Variable>, args: &serde_json::Map<String, Value>, tool: &str) -> Result<()> {
    let key = string_arg(args, tool, "key")?;
    require_non_empty(tool, "key", &key)?;
    let value = string_arg(args, tool, "value")?;
    let secret = bool_arg(args, tool, "secret", false)?;
    let enabled = bool_arg(args, tool, "enabled", true)?;
    let description = optional_string(args, tool, "description")?.unwrap_or_default();
    let existing = variables.iter_mut().find(|variable| variable.key == key);
    match existing {
        Some(variable) => {
            variable.value = value;
            variable.secret = secret;
            variable.enabled = enabled;
            variable.description = description;
        }
        None => variables.push(Variable { key, value, description, enabled, secret, extras: Default::default() }),
    }
    Ok(())
}

fn require_non_empty(tool: &str, argument: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(AutomationError::BadArgument {
            tool: tool.to_string(),
            argument: argument.to_string(),
            expected: "a non-empty string".to_string(),
            got: "an empty string".to_string(),
        });
    }
    Ok(())
}

/// The parent folder id of an item, if any.
fn parent_of(collection: &postguy_model::Collection, id: &str) -> Option<String> {
    fn walk(items: &[Item], id: &str, parent: Option<&str>) -> Option<Option<String>> {
        for item in items {
            if item.id().0 == id {
                return Some(parent.map(str::to_string));
            }
            if let Item::Folder(folder) = item
                && let Some(found) = walk(&folder.items, id, Some(&folder.id.0))
            {
                return Some(found);
            }
        }
        None
    }
    walk(&collection.items, id, None).flatten()
}

/// Park a deleted item's JSON in `<workspace>/.trash/item-<nanos>.json` so a delete is never
/// a silent shred (restore UI arrives with M3's trash view).
fn park_deleted(session: &Session, collection_id: &str, item_id: &str, item: &Item) -> Result<()> {
    let workspace = session.workspace()?;
    let trash = workspace.paths.trash_dir();
    std::fs::create_dir_all(&trash).map_err(|err| AutomationError::Failed { message: format!("creating {}: {err}", trash.display()) })?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|duration| duration.as_nanos()).unwrap_or(0);
    let record = json!({
        "kind": "item",
        "collectionId": collection_id,
        "itemId": item_id,
        "deletedAt": postguy_util::time::now_unix_ms(),
        "item": item,
    });
    let path = trash.join(format!("item-{nanos}.json"));
    let bytes = serde_json::to_vec_pretty(&record).map_err(json_error)?;
    postguy_util::atomic_write(&path, &bytes)?;
    Ok(())
}

fn finish_export(session: &Session, args: &serde_json::Map<String, Value>, value: Value) -> Result<Value> {
    match optional_string(args, "x", "out")? {
        Some(path) => {
            let bytes = serde_json::to_vec_pretty(&value).map_err(json_error)?;
            let written = write_output(session, &path, &bytes)?;
            Ok(json!({"written": written}))
        }
        None => Ok(json!({"json": value})),
    }
}

fn import_text(session: &mut Session, text: &str, format: &str) -> Result<Value> {
    if format != "auto" {
        return Err(AutomationError::Failed { message: format!("format `{format}` is not supported yet; M1 imports Postman JSON (\"auto\")") });
    }
    let parsed = parse_json_str(text, &Caps::default()).map_err(|err| AutomationError::Interop { message: err.to_string() })?;
    let (imported, report) = postguy_interop::import_value(&parsed).map_err(|err| AutomationError::Interop { message: err.to_string() })?;
    let workspace = session.workspace()?;
    let report = serde_json::to_value(&report).map_err(json_error)?;
    match imported {
        postguy_interop::Imported::Collection(collection) => {
            let existing = workspace.load_collection(&collection.id)?;
            if !matches!(existing, postguy_store::LoadOutcome::Fresh(_)) {
                return Err(AutomationError::Failed {
                    message: format!(
                        "a collection with id {} already exists ({}); delete it first or import into another workspace",
                        collection.id.0, collection.name
                    ),
                });
            }
            workspace.save_collection(&collection)?;
            Ok(json!({"kind": "collection", "id": collection.id.0, "name": collection.name, "report": report}))
        }
        postguy_interop::Imported::Environment(environment) => {
            workspace.save_environment(&environment)?;
            Ok(json!({"kind": "environment", "id": environment.id.0, "name": environment.name, "report": report}))
        }
        postguy_interop::Imported::Globals(environment) => {
            workspace.save_globals(&environment)?;
            Ok(json!({"kind": "globals", "name": environment.name, "report": report}))
        }
    }
}

fn json_error(err: serde_json::Error) -> AutomationError {
    AutomationError::Failed { message: err.to_string() }
}
