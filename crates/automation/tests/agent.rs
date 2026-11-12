//! End-to-end: the collection/folder update tools over an engine the caller opened.

use postguy_automation::{Automation, Root};
use postguy_engine::Engine;
use postguy_store::StorePaths;
use serde_json::{Value, json};

struct Fixture {
    _dir: tempfile::TempDir,
    automation: Automation,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical");
    let engine = Engine::open(StorePaths::new(root.clone(), root.join(".postguy-config"))).expect("engine");
    let automation = Automation::with_engine(engine, Some(Root::new(&root).expect("root")));
    Fixture { _dir: dir, automation }
}

fn call(automation: &Automation, tool: &str, args: Value) -> Value {
    automation.call(tool, &args).unwrap_or_else(|err| panic!("{tool} failed: {err}"))
}

fn id(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_else(|| panic!("no `{key}` in {value}")).to_string()
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
