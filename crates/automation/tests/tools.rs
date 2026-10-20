//! End-to-end: the tool table over a real workspace folder — collections, folders, requests,
//! environments with secret splitting, imports/exports, and the confinement contract.

use postguy_automation::{Automation, Root};
use serde_json::{Value, json};

fn automation_in(dir: &std::path::Path) -> Automation {
    let root = Root::new(dir).expect("temp root canonicalizes");
    Automation::new(Some(root))
}

fn call(automation: &Automation, tool: &str, args: Value) -> Value {
    automation.call(tool, &args).unwrap_or_else(|err| panic!("{tool} failed: {err}"))
}

#[test]
fn the_tool_table_round_trips_a_workspace() {
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());

    // Open creates the tree and reports zero counts.
    let info = call(&automation, "workspace_open", json!({"path": "work"}));
    assert_eq!(info["collections"], 0);
    assert_eq!(info["requests"], 0);

    // Collection -> folder -> request, then read back through the same table.
    let collection = call(&automation, "collection_create", json!({"name": "Face Service Rust"}))["collection"].as_str().unwrap().to_string();
    let folder = call(&automation, "folder_create", json!({"collection": collection, "name": "v1"}))["folder"].as_str().unwrap().to_string();
    let request =
        call(&automation, "request_create", json!({"collection": collection, "folder": folder, "kind": "http", "name": "Debug"}))["request"]
            .as_str()
            .unwrap()
            .to_string();
    let got = call(&automation, "request_get", json!({"collection": collection, "request": request}));
    assert_eq!(got["request"]["name"], "Debug");
    assert_eq!(got["request"]["kind"]["type"], "http");

    let info = call(&automation, "workspace_info", json!({}));
    assert_eq!(info["collections"], 1);
    assert_eq!(info["folders"], 1);
    assert_eq!(info["requests"], 1);

    // Update the request's kind-specific definition and its name.
    call(
        &automation,
        "request_update",
        json!({
            "collection": collection,
            "request": request,
            "name": "Debug v2",
            "definition": {"type": "http", "method": "POST", "url": "http://localhost:8080/echo"}
        }),
    );
    let got = call(&automation, "request_get", json!({"collection": collection, "request": request}));
    assert_eq!(got["request"]["kind"]["method"], "POST");

    // Move the request to the collection root and delete the now-empty folder.
    call(&automation, "request_move", json!({"collection": collection, "request": request}));
    call(&automation, "folder_delete", json!({"collection": collection, "folder": folder}));

    // Import the exported Postman JSON into another workspace name: report comes back.
    let exported = call(&automation, "export_collection", json!({"collection": collection, "format": "postman"}));
    assert_eq!(exported["json"]["info"]["name"], "Face Service Rust");
    let exported_path = "exports/face.postman.json";
    call(&automation, "export_collection", json!({"collection": collection, "format": "postman", "out": exported_path}));
    assert!(dir.path().join(exported_path).exists(), "--root-relative `out` lands under the root");

    // Environment variables: secret values split out of the environment file.
    let environment = call(&automation, "env_create", json!({"name": "dev"}))["environment"].as_str().unwrap().to_string();
    call(&automation, "env_set_var", json!({"environment": environment, "key": "apiKey", "value": "hunter2", "secret": true}));
    call(&automation, "env_set_var", json!({"environment": environment, "key": "baseUrl", "value": "http://localhost:8080"}));
    let file = std::fs::read_to_string(dir.path().join("work/environments").join(format!("{environment}.json"))).expect("env file");
    assert!(!file.contains("hunter2"), "secret values never enter the environment file: {file}");
    let loaded = call(&automation, "env_get", json!({"environment": environment}));
    assert_eq!(loaded["environment"]["variables"][0]["value"], "hunter2", "readers see the value rehydrated");
    call(&automation, "env_set_active", json!({"environment": environment}));
    assert_eq!(call(&automation, "workspace_info", json!({}))["activeEnvironmentId"], environment);

    // Globals share the shape.
    call(&automation, "globals_set_var", json!({"key": "region", "value": "eu-west-1"}));
    let globals = call(&automation, "globals_get", json!({}));
    assert_eq!(globals["globals"]["variables"][0]["key"], "region");

    // Delete the collection; it lands in the trash with its item record intact.
    call(&automation, "collection_delete", json!({"collection": collection}));
    let info = call(&automation, "workspace_info", json!({}));
    assert_eq!(info["collections"], 0);
    let trash = dir.path().join("work/.trash");
    assert!(std::fs::read_dir(&trash).expect("trash dir").count() >= 1, "delete parked the trash metadata");
}

#[test]
fn imports_go_through_the_report_and_the_workspace() {
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    call(&automation, "workspace_open", json!({"path": "work"}));

    let collection_json = r#"{
        "info": {"_postman_id": "11111111-2222-3333-4444-555555555555", "name": "Imported", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
        "item": [{"name": "ping", "request": {"method": "GET", "url": {"raw": "http://localhost/ping"}}}]
    }"#;
    let imported = call(&automation, "import_text", json!({"text": collection_json}));
    assert_eq!(imported["kind"], "collection");
    assert_eq!(imported["id"], "11111111-2222-3333-4444-555555555555");
    let second = automation.call("import_text", &json!({"text": collection_json}));
    assert!(second.is_err(), "the same id cannot be imported twice silently: {second:?}");

    let env_json = r#"{
        "id": "env-1", "name": "Imported Env", "_postman_variable_scope": "environment",
        "values": [{"key": "token", "value": "s3cret", "type": "secret", "enabled": true}]
    }"#;
    let imported_env = call(&automation, "import_text", json!({"text": env_json}));
    assert_eq!(imported_env["kind"], "environment");
    let env_dir = dir.path().join("work/environments");
    let mut checked = 0;
    for entry in std::fs::read_dir(&env_dir).expect("environments dir") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let file = std::fs::read_to_string(&path).expect("env file");
            assert!(!file.contains("s3cret"), "imported secrets split out too: {}", path.display());
            checked += 1;
        }
    }
    assert_eq!(checked, 1, "exactly one imported environment file");
}

#[test]
fn root_confinement_refuses_identically_for_existing_and_missing_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("allowed");
    std::fs::create_dir_all(&root).expect("root dir");
    let automation = Automation::new(Some(Root::new(&root).expect("canonical root")));
    // One existing outside file and one nonexistent outside path: same message.
    let outside_existing = dir.path().join("outside.json");
    std::fs::write(&outside_existing, b"{}").expect("outside file");
    let err_missing = automation.call("import_file", &json!({"path": "../ghost.json"})).expect_err("must refuse");
    let err_existing = automation.call("import_file", &json!({"path": "../outside.json"})).expect_err("must refuse");
    let missing = err_missing.to_string();
    let existing = err_existing.to_string();
    assert!(missing.contains("is outside the allowed directory"), "{missing}");
    assert!(existing.contains("is outside the allowed directory"), "{existing}");
    // The paths differ (they are different names), but the FORM is identical: swap the name
    // and the rest of the sentence matches.
    assert_eq!(missing.replace("../ghost.json", "PATH"), existing.replace("../outside.json", "PATH"));
    // The workspace path itself is refused too when outside.
    let refused = automation.call("workspace_open", &json!({"path": "../escape-work"})).expect_err("refused");
    assert!(refused.to_string().contains("is outside the allowed directory"));
}

#[test]
fn unknown_tools_and_arguments_fail_loudly_before_state_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    let err = automation.call("collection_frobnicate", &json!({})).expect_err("unknown");
    assert!(err.to_string().contains("unknown tool `collection_frobnicate`"), "{err}");

    call(&automation, "workspace_open", json!({"path": "work"}));
    let err = automation.call("collection_create", &json!({"name": "x", "nmae": "typo"})).expect_err("unknown arg");
    assert!(err.to_string().contains("unknown argument `nmae`"), "{err}");
    assert!(err.to_string().contains("allowed: name"), "{err}");

    let err = automation.call("collection_create", &json!({})).expect_err("missing arg");
    assert!(err.to_string().contains("missing required argument `name`"), "{err}");

    // Nothing was created by the failed calls.
    let list = call(&automation, "collection_list", json!({}));
    assert_eq!(list["collections"].as_array().map(Vec::len), Some(0));
}
