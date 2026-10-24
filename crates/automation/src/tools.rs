//! The JSON tool table (AGENTS.md 3): every user-facing capability is callable headlessly with
//! an input schema, unknown arguments rejected, and errors that name what is allowed. Tool
//! names are `snake_case`; ids are UUID text from the workspace.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use postguy_engine::{Cancel, ImportOptions, SendOptions, SendTarget};
use postguy_model::{Environment, Id, Item, Variable};
use serde_json::{Value, json};

use crate::Session;
use crate::error::{AutomationError, Result};
use crate::root::Root;
use crate::session::{
    bool_arg, clone_with_new_ids, counts, find_item, find_item_mut, insert_item, json_kind, new_request, optional_string, string_arg, take_item,
    write_output,
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
    /// In-flight sends by request id (and `last`), so `request_cancel` can reach them while
    /// `request_send` blocks in another call.
    cancels: Arc<Mutex<HashMap<String, Cancel>>>,
}

impl Automation {
    /// A fresh automation; `root` confines every path argument when set.
    pub fn new(root: Option<Root>) -> Automation {
        Automation { session: Mutex::new(Session::new(root)), cancels: Arc::new(Mutex::new(HashMap::new())) }
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
        // `request_send` blocks for the whole exchange; it must not hold the session lock, or
        // `request_cancel` from another thread could never reach the in-flight handle.
        if tool_name == "request_send" {
            return self.send_request(tool_name, &args);
        }
        if tool_name == "request_cancel" {
            return self.cancel_request(tool_name, &args);
        }
        if tool_name == "collection_run" {
            return self.run_collection(tool_name, &args);
        }
        let mut session = self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        dispatch(tool_name, &args, &mut session)
    }
}

impl Automation {
    /// `request_send`: resolve the stored request, register a cancellation handle, run the
    /// engine pipeline and return the outcome (the full body is available through
    /// `response_get`/`response_save`; this summary caps it at 1 MiB).
    fn send_request(&self, tool: &str, args: &serde_json::Map<String, Value>) -> Result<Value> {
        let (engine, collection, request, environment) = {
            let session = self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let engine = session.engine()?.clone();
            let collection = string_arg(args, tool, "collection")?;
            let request = string_arg(args, tool, "request")?;
            let environment = optional_string(args, tool, "environment")?.map(Id);
            (engine, Id(collection), Id(request), environment)
        };
        let cancel = Cancel::new();
        if let Ok(mut cancels) = self.cancels.lock() {
            cancels.insert(request.0.clone(), cancel.clone());
            cancels.insert("last".to_string(), cancel.clone());
        }
        let options = SendOptions { environment, cancel: Some(cancel), run_scripts: true, ..SendOptions::default() };
        let outcome = engine.send(SendTarget::Stored { collection, request: request.clone() }, options);
        if let Ok(mut cancels) = self.cancels.lock() {
            cancels.remove(&request.0);
            cancels.remove("last");
        }
        let outcome = outcome?;
        let mut value = cached_json(&postguy_engine::CachedResponse {
            history_id: outcome.history.id.0.clone(),
            request: outcome.request.clone(),
            response: outcome.response.clone(),
            warnings: outcome.warnings.clone(),
            script: outcome.script.clone(),
        });
        if let Some(object) = value.as_object_mut() {
            object.insert("warnings".into(), serde_json::to_value(&outcome.warnings).map_err(json_error)?);
        }
        Ok(value)
    }

    /// `collection_run`: like `request_send`, it must not hold the session lock while it runs,
    /// so `request_cancel run:<collection>` can reach it.
    fn run_collection(&self, tool: &str, args: &serde_json::Map<String, Value>) -> Result<Value> {
        let (engine, config, data) = {
            let session = self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let engine = session.engine()?.clone();
            let config = postguy_engine::RunConfig {
                collection: Id(string_arg(args, tool, "collection")?),
                folder: optional_string(args, tool, "folder")?.map(Id),
                environment: optional_string(args, tool, "environment")?.map(Id),
                iterations: optional_integer(args, tool, "iterations")?
                    .map(|count| {
                        u32::try_from(count).map_err(|_| AutomationError::BadArgument {
                            tool: tool.to_string(),
                            argument: "iterations".to_string(),
                            expected: "an integer between 1 and 10000".to_string(),
                            got: count.to_string(),
                        })
                    })
                    .transpose()?,
                delay_ms: optional_integer(args, tool, "delay_ms")?.map(|ms| u64::try_from(ms).unwrap_or(u64::MAX)).unwrap_or(0),
                stop_on_error: bool_arg(args, tool, "stop_on_error", false)?,
                requests: optional_string_array(args, tool, "requests")?.map(|ids| ids.into_iter().map(Id).collect()),
            };
            let data = match optional_string(args, tool, "data")? {
                Some(given) => {
                    let path = session.resolve_existing(&given)?;
                    Some(postguy_engine::load_data_file(&path)?)
                }
                None => None,
            };
            (engine, config, data)
        };
        let cancel = Cancel::new();
        let key = format!("run:{}", config.collection.0);
        if let Ok(mut cancels) = self.cancels.lock() {
            cancels.insert(key.clone(), cancel.clone());
        }
        let result = engine.run_collection(&config, data, &cancel, &mut |_| {});
        if let Ok(mut cancels) = self.cancels.lock() {
            cancels.remove(&key);
        }
        serde_json::to_value(result?).map_err(json_error)
    }

    /// `request_cancel`: cancel the in-flight send for a request id (or `last`).
    fn cancel_request(&self, tool: &str, args: &serde_json::Map<String, Value>) -> Result<Value> {
        let key = string_arg(args, tool, "sendId")?;
        let cancels = self.cancels.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match cancels.get(&key) {
            Some(cancel) => {
                cancel.cancel();
                Ok(json!({"cancelled": true, "sendId": key}))
            }
            None => Err(AutomationError::Failed {
                message: format!(
                    "no in-flight send `{key}`; use the request id passed to request_send (or `last`), and call request_cancel while the send is running"
                ),
            }),
        }
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

/// An optional integer argument schema.
fn opt_int(description: &'static str, minimum: u64) -> (Value, bool) {
    (json!({"type": "integer", "minimum": minimum, "description": description}), false)
}

/// An optional array-of-strings argument schema.
fn opt_string_array(description: &'static str) -> (Value, bool) {
    (json!({"type": "array", "items": {"type": "string"}, "description": description}), false)
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
            name: "grpc_describe",
            title: "Describe a gRPC service definition",
            description: "Compiles the request's proto set/inline files and lists services, methods, input/output types and streaming flags. Reflection is not implemented yet.",
            input_schema: schema! { "collection": req("collection id"), "request": req("request id") },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "grpc_example_message",
            title: "Example gRPC request message",
            description: "A default JSON object for the request method's input type.",
            input_schema: schema! { "collection": req("collection id"), "request": req("request id") },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "grpc_invoke",
            title: "Invoke a unary gRPC method",
            description: "Calls one unary method with JSON input (defaults to the request's stored message); returns the response JSON and metadata. Streaming arrives later.",
            input_schema: schema! {
                "collection": req("collection id"),
                "request": req("request id"),
                "message": (json!({"type": "object", "description": "request message JSON (defaults to the stored message)"}), false)
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "grpc_stream",
            title: "Invoke a streaming gRPC method",
            description: "Calls a server-, client- or bidi-streaming method with an array of JSON messages and returns every response message in arrival order (cap 10,000). Unary methods use grpc_invoke.",
            input_schema: schema! {
                "collection": req("collection id"),
                "request": req("request id"),
                "messages": (json!({"type": "array", "description": "request messages in order (server streaming uses the first)"}), false)
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "ws_connect",
            title: "Open a WebSocket",
            description: "Connects a saved WebSocket request (variables resolved at connect), returns a session id. Events are read with ws_messages.",
            input_schema: schema! { "collection": req("collection id"), "request": req("request id") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "ws_send",
            title: "Send a WebSocket message",
            description: "Send text or base64 binary to an open session; the echo/event log records it.",
            input_schema: schema! { "session": req("session id"), "text": opt("text message"), "binary_base64": opt("base64 binary message") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "ws_messages",
            title: "Read WebSocket events",
            description: "Events after `since` (default 0) with sequence, direction, kind, text, bytes and close/detail; includes sent, received and network rows.",
            input_schema: schema! { "session": req("session id"), "since": opt("last sequence number already read") },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "ws_disconnect",
            title: "Close a WebSocket",
            description: "Send a close frame and end the session.",
            input_schema: schema! { "session": req("session id"), "code": opt("close code (default 1000)"), "reason": opt("close reason") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "oauth2_token",
            title: "Get or refresh an OAuth 2.0 token",
            description: "Runs client_credentials/password/authorization_code (with optional PKCE verifier) against the request's OAuth 2.0 auth, or refreshes it; stores the tokens in the secrets file and updates the request auth.",
            input_schema: schema! {
                "collection": req("collection id"),
                "request": req("request id"),
                "mode": req("grant | refresh | code"),
                "code": opt("authorization code for mode=code"),
                "verifier": opt("PKCE verifier for mode=code with PKCE")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "snippet_generate",
            title: "Generate a code snippet",
            description: "One HTTP request as cURL, raw HTTP, fetch, Python requests, Go, Rust reqwest, PowerShell or Node axios; variables stay unresolved. gRPC/WS snippets are not implemented yet.",
            input_schema: schema! {
                "collection": req("collection id"),
                "request": req("request id"),
                "language": opt("curl (default), raw-http, fetch, python, go, rust, powershell or node-axios"),
                "indent": (json!({"type": "integer", "minimum": 0, "maximum": 8, "description": "spaces per level; 0 means a tab (default 2)"}), false),
                "include_boilerplate": opt_bool("wrap the request in imports/main (default true)")
            },
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
            title: "Import text",
            description: "Postman collection (v2.0/v2.1), environment or globals JSON, OpenAPI 3 / Swagger 2 (JSON or YAML), or cURL commands; auto-detected, name clashes get a suffix unless `replace` is set; returns the ImportReport.",
            input_schema: schema! {
                "text": req("the JSON/YAML document or cURL command text"),
                "format": opt("\"auto\" (default): the format is detected"),
                "replace": opt_bool("replace a same-id collection/environment or the existing globals instead of suffixing/refusing"),
                "folders": opt("OpenAPI folder layout: \"tags\" (default) or \"paths\"")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "import_file",
            title: "Import a JSON, YAML or cURL file",
            description: "Size-capped (100 MB) read, then the same mapping as import_text; the path resolves inside --root.",
            input_schema: schema! {
                "path": req("file path"),
                "replace": opt_bool("replace instead of suffixing"),
                "folders": opt("OpenAPI folder layout: \"tags\" (default) or \"paths\"")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "import_zip",
            title: "Import a zip of JSON documents",
            description: "Reads .json entries (collection/environment/globals) from a zip through the normal importers; unsafe paths are refused, arbitrary entries are skipped.",
            input_schema: schema! { "path": req("zip file path"), "replace": opt_bool("replace same-id documents instead of suffixing") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "workspace_export_zip",
            title: "Export all workspace data",
            description: "Zip the workspace documents (not .postguy secrets/cookies/cache and not .trash); path resolves inside --root.",
            input_schema: schema! { "out": req("zip file path") },
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
            description: "Postman environment shape; secret values are omitted unless include_secrets is set. Writes `out` or returns the JSON.",
            input_schema: schema! { "environment": req("environment id"), "out": opt("file path to write"), "include_secrets": opt_bool("include secret values") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "export_globals",
            title: "Export globals",
            description: "Postman globals shape; secret values are omitted unless include_secrets is set. Writes `out` or returns the JSON.",
            input_schema: schema! { "out": opt("file path to write"), "include_secrets": opt_bool("include secret values") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "request_send",
            title: "Send a saved request",
            description: "Runs the full send pipeline (variables, inherited auth, cookies, proxy, redirects, decompression) and records History, Console and the response cache.",
            input_schema: schema! {
                "collection": req("collection id"),
                "request": req("request id"),
                "environment": opt("environment id (default: the workspace's active environment)")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "request_cancel",
            title: "Cancel an in-flight send",
            description: "Cancels the send started by request_send for that request id, or the most recent send (`last`), or a collection run (`run:<collection id>`).",
            input_schema: schema! { "sendId": req("request id passed to request_send, `last`, or `run:<collection id>`") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "collection_run",
            title: "Run a collection",
            description: "Runs the collection (or one folder, or an ordered subset of requests) sequentially: scripts, tests, iterations and CSV/JSON data rows, delay, stop on error. gRPC and WebSocket requests are skipped. The result is stored (newest 20 per collection) and returned; cancel it with request_cancel `run:<collection id>`.",
            input_schema: schema! {
                "collection": req("collection id"),
                "folder": opt("folder id: run only this folder"),
                "environment": opt("environment id (default: the workspace's active environment)"),
                "iterations": opt_int("iteration count (default: one per data row, or 1; at most 10000)", 1),
                "delay_ms": opt_int("pause between requests in ms (at most 600000)", 0),
                "data": opt("path of a CSV (header row) or JSON (array of objects) data file, inside --root"),
                "stop_on_error": opt_bool("stop after the first request error, script failure or failed test"),
                "requests": opt_string_array("request ids to run, in this order (default: every request in tree order)")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "run_list",
            title: "List stored runs",
            description: "The collection's stored runs, newest first, with totals and pass/fail.",
            input_schema: schema! { "collection": req("collection id") },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "run_get",
            title: "Get one stored run",
            description: "The full result of a stored run: iterations, requests, tests and failures.",
            input_schema: schema! { "collection": req("collection id"), "run": req("run id from run_list") },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "run_export",
            title: "Export a stored run",
            description: "Renders a stored run as `json`, `junit` (XML) or `html` (one self-contained file) and writes it to `out` (inside --root).",
            input_schema: schema! {
                "collection": req("collection id"),
                "run": req("run id from run_list"),
                "format": req("json, junit or html"),
                "out": req("file path to write")
            },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "response_get",
            title: "Get a finished response",
            description: "The newest (or `id`) cached response: status, headers, body (base64 plus text when UTF-8, capped at 1 MiB), size and timings.",
            input_schema: schema! { "id": opt("history id (omit for the newest response)") },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "response_save",
            title: "Save a response body",
            description: "Writes the DECODED body atomically to `path` inside --root.",
            input_schema: schema! { "id": opt("history id (omit for the newest response)"), "path": req("file path") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "response_render_body",
            title: "Render a response body",
            description: "Returns the body as UTF-8 text (with replacement markers when invalid) or base64, capped at 1 MiB.",
            input_schema: schema! {
                "id": opt("history id (omit for the newest response)"),
                "format": opt("\"text\" (default) or \"base64\"")
            },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "cookies_list",
            title: "List cookies",
            description: "Every jar cookie with domain, path, flags and expiry, plus the script allowlist.",
            input_schema: schema! {},
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "cookies_set",
            title: "Set a cookie",
            description: "Stores one raw Set-Cookie value for a URL (domain/path/secure/expiry rules apply) and persists the jar.",
            input_schema: schema! { "url": req("the URL the cookie belongs to"), "setCookie": req("one Set-Cookie header value") },
            read_only: false,
            destructive: false,
        },
        ToolDef {
            name: "cookies_delete",
            title: "Delete a cookie",
            description: "Deletes by (domain, path, name) and persists the jar.",
            input_schema: schema! { "domain": req("cookie domain"), "path": req("cookie path"), "name": req("cookie name") },
            read_only: false,
            destructive: true,
        },
        ToolDef {
            name: "cookies_clear",
            title: "Clear cookies",
            description: "Deletes every cookie and persists the empty jar.",
            input_schema: schema! {},
            read_only: false,
            destructive: true,
        },
        ToolDef {
            name: "history_list",
            title: "List history",
            description: "The newest sends, newest last (default 50).",
            input_schema: schema! { "limit": (json!({"type": "integer", "minimum": 1}), false) },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "history_open",
            title: "Open a history entry",
            description: "One entry by id, including the as-sent request snapshot.",
            input_schema: schema! { "id": req("history id") },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "history_clear",
            title: "Clear history",
            description: "Empties history.jsonl atomically.",
            input_schema: schema! {},
            read_only: false,
            destructive: true,
        },
        ToolDef {
            name: "console_get",
            title: "Read the console",
            description: "Newest-last console entries, optionally filtered by level or kind (default 100).",
            input_schema: schema! {
                "limit": (json!({"type": "integer", "minimum": 1}), false),
                "level": opt("\"info\" | \"warning\" | \"error\""),
                "kind": opt("\"request\" | \"log\" | \"warning\" | \"error\"")
            },
            read_only: true,
            destructive: false,
        },
        ToolDef {
            name: "console_clear",
            title: "Clear the console",
            description: "Drops every retained console entry (the history file is untouched).",
            input_schema: schema! {},
            read_only: false,
            destructive: true,
        },
        ToolDef {
            name: "variables_resolve",
            title: "Resolve variables",
            description: "Resolves {{variables}} and reports each hit's source scope, secret flag and unresolved state.",
            input_schema: schema! {
                "text": req("template text"),
                "collection": opt("collection id for collection variables"),
                "environment": opt("environment id (default: active)")
            },
            read_only: true,
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
        "grpc_describe" => {
            let collection = session.load_collection(&string_arg(args, tool, "collection")?)?;
            let request_id = string_arg(args, tool, "request")?;
            let item =
                find_item(&collection, &request_id).ok_or_else(|| AutomationError::NotFound { kind: "request".into(), id: request_id.clone() })?;
            let postguy_model::Item::Request(request) = item else {
                return Err(AutomationError::Failed { message: format!("`{request_id}` is a folder, not a request") });
            };
            let postguy_model::RequestKind::Grpc(grpc) = &request.kind else {
                return Err(AutomationError::Failed { message: "grpc_describe expects a gRPC request".into() });
            };
            let prepared =
                session.engine()?.prepare_grpc(&postguy_model::Id(collection.id.0.clone()), &postguy_model::Id(request_id.clone()), grpc)?;
            Ok(json!({"services": session.engine()?.grpc_describe(&prepared)?}))
        }
        "grpc_example_message" => {
            let collection = session.load_collection(&string_arg(args, tool, "collection")?)?;
            let request_id = string_arg(args, tool, "request")?;
            let item =
                find_item(&collection, &request_id).ok_or_else(|| AutomationError::NotFound { kind: "request".into(), id: request_id.clone() })?;
            let postguy_model::Item::Request(request) = item else {
                return Err(AutomationError::Failed { message: format!("`{request_id}` is a folder, not a request") });
            };
            let postguy_model::RequestKind::Grpc(grpc) = &request.kind else {
                return Err(AutomationError::Failed { message: "grpc_example_message expects a gRPC request".into() });
            };
            let prepared =
                session.engine()?.prepare_grpc(&postguy_model::Id(collection.id.0.clone()), &postguy_model::Id(request_id.clone()), grpc)?;
            Ok(json!({"message": session.engine()?.grpc_example(&prepared)?}))
        }
        "grpc_invoke" => {
            let collection = session.load_collection(&string_arg(args, tool, "collection")?)?;
            let request_id = string_arg(args, tool, "request")?;
            let item =
                find_item(&collection, &request_id).ok_or_else(|| AutomationError::NotFound { kind: "request".into(), id: request_id.clone() })?;
            let postguy_model::Item::Request(request) = item else {
                return Err(AutomationError::Failed { message: format!("`{request_id}` is a folder, not a request") });
            };
            let postguy_model::RequestKind::Grpc(grpc) = &request.kind else {
                return Err(AutomationError::Failed { message: "grpc_invoke expects a gRPC request".into() });
            };
            let grpc = &session.engine()?.prepare_grpc(&postguy_model::Id(collection.id.0.clone()), &postguy_model::Id(request_id.clone()), grpc)?;
            let message = match args.get("message") {
                Some(Value::Object(_)) => args.get("message").cloned().unwrap_or_default(),
                _ => serde_json::from_str(&grpc.message).map_err(|err| AutomationError::BadArgument {
                    tool: tool.to_string(),
                    argument: "message".into(),
                    expected: "a JSON object".into(),
                    got: err.to_string(),
                })?,
            };
            let result = session.engine()?.grpc_invoke_unary(grpc, message)?;
            Ok(json!({"message": result.json, "metadata": result.metadata, "status": result.status}))
        }
        "grpc_stream" => {
            let collection = session.load_collection(&string_arg(args, tool, "collection")?)?;
            let request_id = string_arg(args, tool, "request")?;
            let item =
                find_item(&collection, &request_id).ok_or_else(|| AutomationError::NotFound { kind: "request".into(), id: request_id.clone() })?;
            let postguy_model::Item::Request(request) = item else {
                return Err(AutomationError::Failed { message: format!("`{request_id}` is a folder, not a request") });
            };
            let postguy_model::RequestKind::Grpc(grpc) = &request.kind else {
                return Err(AutomationError::Failed { message: "grpc_stream expects a gRPC request".into() });
            };
            let grpc = &session.engine()?.prepare_grpc(&postguy_model::Id(collection.id.0.clone()), &postguy_model::Id(request_id.clone()), grpc)?;
            let messages = match args.get("messages") {
                Some(Value::Array(messages)) => messages.clone(),
                _ => serde_json::from_str::<Vec<serde_json::Value>>(&grpc.message).map_err(|err| AutomationError::BadArgument {
                    tool: tool.to_string(),
                    argument: "messages".into(),
                    expected: "an array of JSON messages (or store one array in the request message)".into(),
                    got: err.to_string(),
                })?,
            };
            let result = session.engine()?.grpc_stream(grpc, messages)?;
            Ok(json!({"messages": result.messages, "metadata": result.metadata, "status": result.status}))
        }
        "ws_connect" => {
            let collection_id = string_arg(args, tool, "collection")?;
            let request_id = string_arg(args, tool, "request")?;
            let collection = session.load_collection(&collection_id)?;
            let request =
                find_item(&collection, &request_id).ok_or_else(|| AutomationError::NotFound { kind: "request".into(), id: request_id.clone() })?;
            let postguy_model::Item::Request(request) = request else {
                return Err(AutomationError::Failed { message: format!("`{request_id}` is a folder, not a request") });
            };
            if !matches!(request.kind, postguy_model::RequestKind::WebSocket(_)) {
                return Err(AutomationError::Failed { message: "ws_connect expects a WebSocket request".into() });
            }
            let id = session.engine()?.ws_connect_saved(&collection.id, &request.id)?;
            Ok(json!({"session": id, "connected": false}))
        }
        "ws_send" => {
            let session_id = string_arg(args, tool, "session")?;
            let text = optional_string(args, tool, "text")?;
            let binary = optional_string(args, tool, "binary_base64")?;
            let engine = session.engine()?;
            match (text, binary) {
                (Some(text), None) => engine.ws_send_text(&session_id, text)?,
                (None, Some(binary)) => {
                    let bytes = base64::engine::general_purpose::STANDARD.decode(binary).map_err(|err| AutomationError::BadArgument {
                        tool: tool.to_string(),
                        argument: "binary_base64".into(),
                        expected: "base64".into(),
                        got: err.to_string(),
                    })?;
                    engine.ws_send_binary(&session_id, bytes)?;
                }
                (Some(_), Some(_)) => return Err(AutomationError::Failed { message: "ws_send takes text or binary_base64, not both".into() }),
                (None, None) => return Err(AutomationError::MissingArgument { tool: tool.to_string(), argument: "text or binary_base64".into() }),
            }
            Ok(json!({"sent": true}))
        }
        "ws_messages" => {
            let session_id = string_arg(args, tool, "session")?;
            let since = args.get("since").and_then(Value::as_u64).unwrap_or(0);
            let engine = session.engine()?;
            Ok(json!({"events": engine.ws_events(&session_id, since)?, "connected": engine.ws_connected(&session_id)?}))
        }
        "ws_disconnect" => {
            let session_id = string_arg(args, tool, "session")?;
            let code = args.get("code").and_then(Value::as_u64).unwrap_or(1000).min(4999) as u16;
            let reason = optional_string(args, tool, "reason")?.unwrap_or_default();
            session.engine()?.ws_disconnect(&session_id, code, reason)?;
            Ok(json!({"closed": true}))
        }
        "oauth2_token" => {
            let collection_id = string_arg(args, tool, "collection")?;
            let request_id = string_arg(args, tool, "request")?;
            let mode = string_arg(args, tool, "mode")?;
            let mut collection = session.load_collection(&collection_id)?;
            let request = match find_item_mut(&mut collection, &request_id) {
                Some(Item::Request(request)) => request,
                Some(Item::Folder(_)) => return Err(AutomationError::Failed { message: format!("`{request_id}` is a folder, not a request") }),
                None => return Err(AutomationError::NotFound { kind: "request".into(), id: request_id }),
            };
            let postguy_model::RequestKind::Http(http) = &mut request.kind else {
                return Err(AutomationError::Failed { message: "OAuth 2.0 tools apply to HTTP requests".into() });
            };
            let postguy_model::AuthConfig::OAuth2(auth) = &http.auth else {
                return Err(AutomationError::Failed { message: "the request's auth is not OAuth 2.0".into() });
            };
            let mut auth = (**auth).clone();
            let engine = session.engine()?;
            let token = match mode.as_str() {
                "grant" => engine.oauth2_grant(&auth),
                "refresh" => {
                    let refresh = auth.refresh_token.clone();
                    engine.oauth2_refresh(&auth, &refresh)
                }
                "code" => {
                    let code = optional_string(args, tool, "code")?
                        .ok_or_else(|| AutomationError::MissingArgument { tool: tool.to_string(), argument: "code".into() })?;
                    let verifier = optional_string(args, tool, "verifier")?;
                    engine.oauth2_exchange_code(&auth, &code, verifier.as_deref())
                }
                other => {
                    return Err(AutomationError::BadArgument {
                        tool: tool.to_string(),
                        argument: "mode".into(),
                        expected: "grant, refresh or code".into(),
                        got: other.to_string(),
                    });
                }
            }
            .map_err(|err| AutomationError::Failed { message: err.to_string() })?;
            let now = postguy_util::time::now_unix_ms();
            let expires_at_ms =
                token.expires_in.map(|seconds| now.saturating_add(i64::try_from(seconds.saturating_mul(1000)).unwrap_or(i64::MAX))).unwrap_or(0);
            auth.access_token = token.access_token.clone();
            auth.access_token_expires_at_ms = expires_at_ms;
            if !token.refresh_token.is_empty() {
                auth.refresh_token = token.refresh_token.clone();
            }
            if !token.scope.is_empty() {
                auth.scope = token.scope.clone();
            }
            http.auth = postguy_model::AuthConfig::OAuth2(Box::new(auth));
            session.workspace()?.save_collection(&collection)?;
            Ok(json!({
                "access_token": token.access_token,
                "token_type": token.token_type,
                "expires_in": token.expires_in,
                "refresh_token": token.refresh_token,
                "scope": token.scope,
                "expires_at_ms": expires_at_ms,
            }))
        }
        "snippet_generate" => {
            let collection = session.load_collection(&string_arg(args, tool, "collection")?)?;
            let request_id = string_arg(args, tool, "request")?;
            let language_id = optional_string(args, tool, "language")?.unwrap_or_else(|| "curl".into());
            let language = postguy_snippets::Language::parse(&language_id).ok_or_else(|| AutomationError::BadArgument {
                tool: tool.to_string(),
                argument: "language".into(),
                expected: format!("one of {}", postguy_snippets::Language::all().iter().map(|language| language.id()).collect::<Vec<_>>().join(", ")),
                got: language_id,
            })?;
            let indent = match args.get("indent") {
                None | Some(Value::Null) => 2,
                Some(Value::Number(value)) => value.as_u64().unwrap_or(2).min(8) as usize,
                Some(other) => {
                    return Err(AutomationError::BadArgument {
                        tool: tool.to_string(),
                        argument: "indent".into(),
                        expected: "an integer from 0 to 8".into(),
                        got: json_kind(other).to_string(),
                    });
                }
            };
            let include_boilerplate = bool_arg(args, tool, "include_boilerplate", true)?;
            match find_item(&collection, &request_id) {
                Some(Item::Request(request)) => match &request.kind {
                    postguy_model::RequestKind::Http(http) => {
                        let snippet = postguy_snippets::generate(language, http, postguy_snippets::Options { indent, include_boilerplate });
                        Ok(json!({"language": language.id(), "snippet": snippet}))
                    }
                    _ => Err(AutomationError::Failed {
                        message: "snippet_generate supports HTTP requests only; gRPC/WS snippets are not implemented yet".into(),
                    }),
                },
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
            let replace = bool_arg(args, tool, "replace", false)?;
            let folders = folders_arg(args, tool)?;
            import_text(session, &text, &format, ImportOptions { replace, folders })
        }
        "import_file" => {
            let path = string_arg(args, tool, "path")?;
            let resolved = session.resolve_existing(&path)?;
            let replace = bool_arg(args, tool, "replace", false)?;
            let folders = folders_arg(args, tool)?;
            let outcome = session.engine()?.import_file(&resolved, ImportOptions { replace, folders })?;
            serde_json::to_value(outcome).map_err(json_error)
        }
        "import_zip" => {
            let path = string_arg(args, tool, "path")?;
            let resolved = session.resolve_existing(&path)?;
            let replace = bool_arg(args, tool, "replace", false)?;
            let outcome = session.engine()?.import_zip(&resolved, ImportOptions { replace, ..ImportOptions::default() })?;
            serde_json::to_value(outcome).map_err(json_error)
        }
        "workspace_export_zip" => {
            let out = string_arg(args, tool, "out")?;
            let path = session.resolve(&out)?;
            let bytes = session.engine()?.export_workspace_zip(&path)?;
            Ok(json!({"written": path.to_string_lossy(), "bytes": bytes}))
        }
        "export_collection" => {
            let id = string_arg(args, tool, "collection")?;
            let format = optional_string(args, tool, "format")?.unwrap_or_else(|| "postman".to_string());
            let value = match format.as_str() {
                "postman" => session.engine()?.export_collection_value(&Id(id), postguy_engine::CollectionFormat::Postman)?,
                "native" => session.engine()?.export_collection_value(&Id(id), postguy_engine::CollectionFormat::Native)?,
                other => {
                    return Err(AutomationError::BadArgument {
                        tool: tool.to_string(),
                        argument: "format".to_string(),
                        expected: "\"postman\" or \"native\"".to_string(),
                        got: other.to_string(),
                    });
                }
            };
            finish_export(tool, session, args, value)
        }
        "export_environment" => {
            let id = string_arg(args, tool, "environment")?;
            let include_secrets = bool_arg(args, tool, "include_secrets", false)?;
            let value = session.engine()?.export_environment_value(&Id(id), include_secrets)?;
            finish_export(tool, session, args, value)
        }
        "export_globals" => {
            let include_secrets = bool_arg(args, tool, "include_secrets", false)?;
            let value = session.engine()?.export_globals_value(include_secrets)?;
            finish_export(tool, session, args, value)
        }
        "run_list" => {
            let runs = session.engine()?.runs(&Id(string_arg(args, tool, "collection")?))?;
            Ok(json!({ "runs": runs }))
        }
        "run_get" => {
            let result = session.engine()?.run(&Id(string_arg(args, tool, "collection")?), &string_arg(args, tool, "run")?)?;
            serde_json::to_value(result).map_err(json_error)
        }
        "run_export" => {
            let format = string_arg(args, tool, "format")?;
            let out = string_arg(args, tool, "out")?;
            let result = session.engine()?.run(&Id(string_arg(args, tool, "collection")?), &string_arg(args, tool, "run")?)?;
            let bytes = match format.as_str() {
                "json" => serde_json::to_vec_pretty(&postguy_runner::report::json(&result)).map_err(json_error)?,
                "junit" => postguy_runner::report::junit(&result).into_bytes(),
                "html" => postguy_runner::report::html(&result).into_bytes(),
                other => {
                    return Err(AutomationError::BadArgument {
                        tool: tool.to_string(),
                        argument: "format".to_string(),
                        expected: "json, junit or html".to_string(),
                        got: other.to_string(),
                    });
                }
            };
            let written = write_output(session, &out, &bytes)?;
            Ok(json!({ "written": written, "format": format, "run": result.id }))
        }
        "response_get" => {
            let engine = session.engine()?;
            let id = optional_string(args, tool, "id")?;
            engine
                .response(id.as_deref())
                .map(|cached| cached_json(&cached))
                .ok_or_else(|| AutomationError::NotFound { kind: "response".into(), id: id.unwrap_or_else(|| "<latest>".into()) })
        }
        "response_save" => {
            let path = string_arg(args, tool, "path")?;
            let cached = {
                let engine = session.engine()?;
                let id = optional_string(args, tool, "id")?;
                engine
                    .response(id.as_deref())
                    .ok_or_else(|| AutomationError::NotFound { kind: "response".into(), id: id.unwrap_or_else(|| "<latest>".into()) })?
            };
            let written = write_output(session, &path, &cached.response.body)?;
            Ok(json!({"written": written, "bytes": cached.response.body.len()}))
        }
        "response_render_body" => {
            let engine = session.engine()?;
            let id = optional_string(args, tool, "id")?;
            let cached = engine
                .response(id.as_deref())
                .ok_or_else(|| AutomationError::NotFound { kind: "response".into(), id: id.unwrap_or_else(|| "<latest>".into()) })?;
            let format = optional_string(args, tool, "format")?.unwrap_or_else(|| "text".to_string());
            let body = &cached.response.body;
            let shown = body.get(..body.len().min(TOOL_BODY_CAP)).unwrap_or(body);
            match format.as_str() {
                "text" => Ok(json!({
                    "format": "text",
                    "body": String::from_utf8_lossy(shown),
                    "bodyLength": body.len(),
                    "truncated": body.len() > shown.len(),
                })),
                "base64" => Ok(json!({
                    "format": "base64",
                    "bodyBase64": base64::engine::general_purpose::STANDARD.encode(shown),
                    "bodyLength": body.len(),
                    "truncated": body.len() > shown.len(),
                })),
                other => Err(AutomationError::BadArgument {
                    tool: tool.to_string(),
                    argument: "format".to_string(),
                    expected: "\"text\" or \"base64\"".to_string(),
                    got: other.to_string(),
                }),
            }
        }
        "cookies_list" => {
            let engine = session.engine()?;
            Ok(json!({
                "cookies": engine.cookies(),
                "allowlist": engine.cookie_domains(),
            }))
        }
        "cookies_set" => {
            let url = string_arg(args, tool, "url")?;
            let set_cookie = string_arg(args, tool, "setCookie")?;
            session.engine()?.set_cookie(&url, &set_cookie)?;
            Ok(json!({"saved": true}))
        }
        "cookies_delete" => {
            let domain = string_arg(args, tool, "domain")?;
            let path = string_arg(args, tool, "path")?;
            let name = string_arg(args, tool, "name")?;
            session.engine()?.remove_cookie(&domain, &path, &name)?;
            Ok(json!({"deleted": true}))
        }
        "cookies_clear" => {
            session.engine()?.clear_cookies()?;
            Ok(json!({"cleared": true}))
        }
        "history_list" => {
            let engine = session.engine()?;
            let limit = optional_integer(args, tool, "limit")?.unwrap_or(50).max(1);
            Ok(json!({"entries": engine.history(limit)?}))
        }
        "history_open" => {
            let engine = session.engine()?;
            let id = string_arg(args, tool, "id")?;
            let entries = engine.history(usize::MAX)?;
            entries
                .into_iter()
                .find(|entry| entry.id.0 == id)
                .map(|entry| json!({"entry": entry}))
                .ok_or(AutomationError::NotFound { kind: "history entry".into(), id })
        }
        "history_clear" => {
            session.engine()?.clear_history()?;
            Ok(json!({"cleared": true}))
        }
        "console_get" => {
            let engine = session.engine()?;
            let limit = optional_integer(args, tool, "limit")?.unwrap_or(100).max(1);
            let level = optional_string(args, tool, "level")?;
            let kind = optional_string(args, tool, "kind")?;
            let entries: Vec<Value> = engine
                .console_entries(limit)
                .into_iter()
                .filter_map(|entry| serde_json::to_value(entry).ok())
                .filter(|value| {
                    level.as_ref().is_none_or(|wanted| value["level"] == Value::String(wanted.clone()))
                        && kind.as_ref().is_none_or(|wanted| value["kind"] == Value::String(wanted.clone()))
                })
                .collect();
            Ok(json!({"entries": entries}))
        }
        "console_clear" => {
            session.engine()?.clear_console();
            Ok(json!({"cleared": true}))
        }
        "variables_resolve" => {
            let text = string_arg(args, tool, "text")?;
            let collection = optional_string(args, tool, "collection")?.map(Id);
            let environment = optional_string(args, tool, "environment")?.map(Id);
            let resolution = session.engine()?.resolve_variables_with(&text, collection.as_ref(), environment.as_ref())?;
            let hits: Vec<Value> = resolution
                .hits
                .iter()
                .map(|hit| {
                    json!({
                        "name": hit.name,
                        "source": match hit.source {
                            postguy_vars::VarSource::Dynamic => "dynamic".to_string(),
                            postguy_vars::VarSource::Scope(kind) => match kind {
                                postguy_vars::ScopeKind::Globals => "globals",
                                postguy_vars::ScopeKind::Collection => "collection",
                                postguy_vars::ScopeKind::Folder => "folder",
                                postguy_vars::ScopeKind::Environment => "environment",
                                postguy_vars::ScopeKind::Data => "data",
                                postguy_vars::ScopeKind::Local => "local",
                            }
                            .to_string(),
                        },
                        "secret": hit.is_secret,
                        "unresolved": hit.unresolved,
                    })
                })
                .collect();
            Ok(json!({"text": resolution.text, "hits": hits}))
        }
        other => Err(AutomationError::UnknownTool { tool: other.to_string() }),
    }
}

/// Tools cap the body they echo (the full body is on disk through `response_save`).
const TOOL_BODY_CAP: usize = 1 << 20;

/// The JSON shape `request_send` and `response_get` share.
fn cached_json(cached: &postguy_engine::CachedResponse) -> Value {
    let body = &cached.response.body;
    let shown = body.get(..body.len().min(TOOL_BODY_CAP)).unwrap_or(body);
    let headers: Vec<Value> = cached.response.headers.iter().map(|(name, value)| json!({"name": name, "value": value})).collect();
    json!({
        "historyId": cached.history_id,
        "request": cached.request,
        "response": {
            "status": cached.response.status,
            "statusText": cached.response.status_text,
            "httpVersion": cached.response.http_version,
            "headers": headers,
            "bodyBase64": base64::engine::general_purpose::STANDARD.encode(shown),
            "bodyText": String::from_utf8(shown.to_vec()).ok(),
            "bodyLength": body.len(),
            "truncated": body.len() > shown.len(),
            "size": cached.response.size,
            "timings": cached.response.timings,
        },
        "warnings": cached.warnings,
        "script": cached.script,
    })
}

fn optional_string_array(args: &serde_json::Map<String, Value>, tool: &str, key: &str) -> Result<Option<Vec<String>>> {
    let bad = |got: &str| AutomationError::BadArgument {
        tool: tool.to_string(),
        argument: key.to_string(),
        expected: "an array of request ids".to_string(),
        got: got.to_string(),
    };
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            items.iter().map(|item| item.as_str().map(str::to_string).ok_or_else(|| bad(json_kind(item)))).collect::<Result<Vec<String>>>().map(Some)
        }
        Some(other) => Err(bad(json_kind(other))),
    }
}

fn optional_integer(args: &serde_json::Map<String, Value>, tool: &str, key: &str) -> Result<Option<usize>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            number.as_u64().and_then(|value| usize::try_from(value).ok()).map(Some).ok_or_else(|| AutomationError::BadArgument {
                tool: tool.to_string(),
                argument: key.to_string(),
                expected: "a non-negative integer".to_string(),
                got: number.to_string(),
            })
        }
        Some(other) => Err(AutomationError::BadArgument {
            tool: tool.to_string(),
            argument: key.to_string(),
            expected: "a non-negative integer".to_string(),
            got: crate::session::json_kind(other).to_string(),
        }),
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
/// a silent shred (the trash view restores it).
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

fn finish_export(tool: &str, session: &Session, args: &serde_json::Map<String, Value>, value: Value) -> Result<Value> {
    match optional_string(args, tool, "out")? {
        Some(path) => {
            let bytes = serde_json::to_vec_pretty(&value).map_err(json_error)?;
            let written = write_output(session, &path, &bytes)?;
            Ok(json!({"written": written}))
        }
        None => Ok(json!({"json": value})),
    }
}

/// The OpenAPI folder layout: `tags` (default) or `paths`.
fn folders_arg(args: &serde_json::Map<String, Value>, tool: &str) -> Result<postguy_interop::FolderStrategy> {
    match optional_string(args, tool, "folders")?.as_deref() {
        None | Some("tags") => Ok(postguy_interop::FolderStrategy::Tags),
        Some("paths") => Ok(postguy_interop::FolderStrategy::Paths),
        Some(other) => Err(AutomationError::BadArgument {
            tool: tool.to_string(),
            argument: "folders".to_string(),
            expected: "tags or paths".to_string(),
            got: other.to_string(),
        }),
    }
}

fn import_text(session: &mut Session, text: &str, format: &str, options: ImportOptions) -> Result<Value> {
    if format != "auto" {
        return Err(AutomationError::Failed { message: format!("format `{format}` is not supported; use \"auto\" (OpenAPI and HAR arrive later)") });
    }
    let outcome = session.engine()?.import_text(text, options)?;
    serde_json::to_value(outcome).map_err(json_error)
}

fn json_error(err: serde_json::Error) -> AutomationError {
    AutomationError::Failed { message: err.to_string() }
}
