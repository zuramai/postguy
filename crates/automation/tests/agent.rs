//! End-to-end: the collection/folder update tools and the AI Agent loop over an engine the caller
//! opened, against the testkit's scripted OpenAI-compatible provider.

use std::sync::Arc;

use postguy_automation::{Automation, Root};
use postguy_engine::Engine;
use postguy_store::StorePaths;
use postguy_store::keychain::MemoryKeyStore;
use postguy_testkit::Server;
use serde_json::{Value, json};

struct Fixture {
    root: std::path::PathBuf,
    server: Server,
    automation: Automation,
    _dir: tempfile::TempDir,
}

/// A workspace at the root of a temp dir, an in-memory keychain and the agent pointed at the
/// testkit provider with the key it accepts.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical");
    let engine = Engine::open(StorePaths::new(root.clone(), root.join(".postguy-config"))).expect("engine");
    engine.set_key_store(Arc::new(MemoryKeyStore::default()));
    let automation = Automation::with_engine(engine, Some(Root::new(&root).expect("root")));
    let server = Server::start();
    call(
        &automation,
        "agent_settings_set",
        json!({"provider": "openai-compatible", "endpoint": format!("{}/ai/v1", server.base_url()), "model": "fake", "api_key": "test-key"}),
    );
    Fixture { root, server, automation, _dir: dir }
}

fn call(automation: &Automation, tool: &str, args: Value) -> Value {
    automation.call(tool, &args).unwrap_or_else(|err| panic!("{tool} failed: {err}"))
}

fn id(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_else(|| panic!("no `{key}` in {value}")).to_string()
}

/// A user message the fake provider turns into one tool call.
fn directive(tool: &str, args: Value) -> String {
    json!({"call": tool, "args": args}).to_string()
}

fn ask(automation: &Automation, tool: &str, args: Value, allow_destructive: bool) -> Value {
    call(automation, "agent_chat", json!({"message": directive(tool, args), "allow_destructive": allow_destructive}))
}

fn collection_names(automation: &Automation) -> Vec<String> {
    call(automation, "collection_list", json!({}))["collections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect()
}

fn last_tool_content(automation: &Automation) -> Value {
    let messages = call(automation, "agent_chat_get", json!({}))["messages"].as_array().unwrap().clone();
    let content = messages.iter().rev().find(|m| m["role"] == "tool").expect("a tool message")["content"].as_str().unwrap().to_string();
    serde_json::from_str(&content).expect("tool content is JSON")
}

#[test]
fn collection_and_folder_update_replace_only_given_fields() {
    let fx = fixture();
    let a = &fx.automation;
    let collection = id(&call(a, "collection_create", json!({"name": "Shop"})), "collection");
    let folder = id(&call(a, "folder_create", json!({"collection": collection, "name": "Orders"})), "folder");
    let request = id(&call(a, "request_create", json!({"collection": collection, "kind": "http", "name": "List"})), "request");

    call(
        a,
        "collection_update",
        json!({
            "collection": collection,
            "variables": [{"key": "baseUrl", "value": "http://localhost:3000"}],
            "auth": {"kind": "bearer", "token": "{{token}}"},
            "scripts": {"prerequest": {"text": "pm.variables.set('a', 1);"}}
        }),
    );
    let got = call(a, "collection_get", json!({"collection": collection}))["collection"].clone();
    assert_eq!(got["name"], "Shop", "fields left out stay as they are");
    assert_eq!(got["variables"][0]["key"], "baseUrl");
    assert_eq!(got["auth"]["kind"], "bearer");
    assert_eq!(got["scripts"]["prerequest"]["text"], "pm.variables.set('a', 1);");

    call(a, "collection_update", json!({"collection": collection, "auth": null, "description": "Docs"}));
    let got = call(a, "collection_get", json!({"collection": collection}))["collection"].clone();
    assert!(got.get("auth").is_none(), "null removes the auth");
    assert_eq!(got["description"], "Docs");
    assert_eq!(got["variables"][0]["key"], "baseUrl");

    call(
        a,
        "folder_update",
        json!({"collection": collection, "folder": folder, "description": "All orders", "scripts": {"test": {"text": "pm.test('ok', () => {});"}}}),
    );
    let got = call(a, "folder_get", json!({"collection": collection, "folder": folder}))["folder"].clone();
    assert_eq!(got["name"], "Orders");
    assert_eq!(got["description"], "All orders");
    assert_eq!(got["scripts"]["test"]["text"], "pm.test('ok', () => {});");

    let error = a.call("folder_get", &json!({"collection": collection, "folder": request})).expect_err("a request is not a folder").to_string();
    assert!(error.contains("is a request, not a folder"), "{error}");
    let error = a.call("folder_update", &json!({"collection": collection, "folder": folder, "name": " "})).expect_err("empty name").to_string();
    assert!(error.contains("name"), "{error}");
    let error = a.call("collection_update", &json!({"collection": collection, "variables": "x"})).expect_err("bad variables").to_string();
    assert!(error.contains("a serialized list of Variable objects"), "{error}");
}

#[test]
fn agent_chat_runs_a_tool_and_persists_the_turn() {
    let fx = fixture();
    let a = &fx.automation;
    let outcome = ask(a, "collection_create", json!({"name": "From agent"}), false);
    assert!(collection_names(a).contains(&"From agent".to_string()));
    assert!(outcome["reply"].as_str().unwrap().starts_with("done:"), "{outcome}");
    assert_eq!(outcome["steps"][0]["tool"], "collection_create");
    assert_eq!(outcome["steps"][0]["ok"], true);
    assert_eq!(outcome["steps"][0]["label"], "Create a collection · \"From agent\"");

    let messages = call(a, "agent_chat_get", json!({}))["messages"].as_array().unwrap().clone();
    let roles: Vec<&str> = messages.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);

    call(a, "agent_chat_clear", json!({}));
    assert_eq!(call(a, "agent_chat_get", json!({}))["messages"], json!([]));
}

#[test]
fn destructive_calls_are_declined_unless_allowed() {
    let fx = fixture();
    let a = &fx.automation;
    let collection = id(&call(a, "collection_create", json!({"name": "From agent"})), "collection");

    let outcome = ask(a, "collection_delete", json!({"collection": collection}), false);
    assert_eq!(outcome["declined"][0], "Delete collection “From agent”");
    assert_eq!(outcome["steps"][0]["ok"], false);
    assert!(collection_names(a).contains(&"From agent".to_string()), "a declined delete changes nothing");
    assert!(last_tool_content(a)["note"].as_str().unwrap().contains("Do not retry"));

    let outcome = ask(a, "collection_delete", json!({"collection": collection}), true);
    assert_eq!(outcome["declined"], json!([]));
    assert!(!collection_names(a).contains(&"From agent".to_string()));
}

#[test]
fn a_run_returns_a_runner_link_and_exports_stay_inside_the_root() {
    let fx = fixture();
    let a = &fx.automation;
    let collection = id(&call(a, "collection_create", json!({"name": "Runs"})), "collection");
    let request = id(&call(a, "request_create", json!({"collection": collection, "kind": "http", "name": "Ok"})), "request");
    let mut definition = call(a, "request_get", json!({"collection": collection, "request": request}))["request"]["kind"].clone();
    definition["url"] = json!(format!("{}/status/200", fx.server.base_url()));
    definition["method"] = json!("GET");
    call(a, "request_update", json!({"collection": collection, "request": request, "definition": definition}));

    let outcome = ask(a, "collection_run", json!({"collection": collection}), false);
    let link = outcome["links"][0]["url"].as_str().expect("a run link").to_string();
    let run = outcome["links"][0]["run"].as_str().unwrap().to_string();
    assert_eq!(link, format!("postguy://run/{collection}/{run}"));
    assert_eq!(last_tool_content(a)["link"], link.as_str(), "the model sees the same link");

    let outside = json!({"collection": collection, "run": run, "format": "csv", "out": "../outside.csv"});
    let direct = a.call("run_export", &outside).expect_err("outside the root").to_string();
    ask(a, "run_export", outside, false);
    assert_eq!(last_tool_content(a)["error"], direct.as_str());
    assert!(!fx.root.parent().unwrap().join("outside.csv").exists());

    let outcome = ask(a, "run_export", json!({"collection": collection, "run": run, "format": "csv", "out": "exports/r.csv"}), false);
    assert_eq!(outcome["steps"][0]["ok"], true, "{outcome}");
    assert!(fx.root.join("exports/r.csv").exists());

    let outcome = ask(a, "run_export", json!({"collection": collection, "run": run, "format": "csv", "out": "exports/r.csv"}), false);
    assert_eq!(outcome["declined"][0], "Overwrite exports/r.csv", "overwriting a file needs approval");
}

#[test]
fn agent_chat_without_a_key_names_the_setting_and_leaves_the_chat_alone() {
    let fx = fixture();
    let a = &fx.automation;
    call(a, "agent_settings_set", json!({"remove_api_key": true}));
    let error = a.call("agent_chat", &json!({"message": "hi"})).expect_err("no key").to_string();
    assert!(error.contains("Settings → AI Agent"), "{error}");
    assert_eq!(call(a, "agent_chat_get", json!({}))["messages"], json!([]));
}

#[test]
fn the_api_key_never_lands_in_settings_json() {
    let fx = fixture();
    let a = &fx.automation;
    let settings = std::fs::read_to_string(fx.root.join(".postguy-config/settings.json")).expect("settings file");
    assert!(!settings.contains("test-key"));
    let got = call(a, "agent_settings_get", json!({}));
    assert_eq!(got["apiKeyStored"], true);
    assert_eq!(got["model"], "fake");
    assert!(!got.to_string().contains("test-key"));
    let error = a.call("agent_settings_set", &json!({"provider": "mistral"})).expect_err("unknown provider").to_string();
    assert!(error.contains("openai-compatible, claude, gemini or openrouter"), "{error}");
    let error = a.call("agent_settings_set", &json!({"endpoint": "ftp://x"})).expect_err("bad endpoint").to_string();
    assert!(error.contains("http://"), "{error}");
}
