//! End-to-end: the tool table over a real workspace folder — collections, folders, requests,
//! environments with secret splitting, imports/exports, and the confinement contract.

use postguy_automation::{Automation, Root};
use serde_json::{Value, json};
use std::io::Write as _;

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
    let snippet = call(
        &automation,
        "snippet_generate",
        json!({"collection": collection, "request": request, "language": "python", "include_boilerplate": false}),
    );
    assert_eq!(snippet["language"], "python");
    assert!(snippet["snippet"].as_str().is_some_and(|text| text.contains("requests.post")), "{}", snippet["snippet"]);
    let bad = automation.call("snippet_generate", &json!({"collection": collection, "request": request, "language": "cobol"}));
    assert!(bad.is_err(), "unknown languages are refused");

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
    let hidden = call(&automation, "export_environment", json!({"environment": environment}));
    assert_eq!(hidden["json"]["values"][0]["value"], "", "secret values are omitted by default");
    let shown = call(&automation, "export_environment", json!({"environment": environment, "include_secrets": true}));
    assert_eq!(shown["json"]["values"][0]["value"], "hunter2", "the explicit opt-in includes the secret");
    let zip = call(&automation, "workspace_export_zip", json!({"out": "exports/workspace.zip"}));
    assert!(zip["bytes"].as_u64().is_some_and(|bytes| bytes > 0));
    assert!(dir.path().join("exports/workspace.zip").exists());

    // Globals share the shape.
    call(&automation, "globals_set_var", json!({"key": "region", "value": "eu-west-1"}));
    let globals = call(&automation, "globals_get", json!({}));
    assert_eq!(globals["globals"]["variables"][0]["key"], "region");

    // Zip import reads JSON documents, applies clash suffixes and skips unrelated entries.
    let zip_path = dir.path().join("incoming.zip");
    let file = std::fs::File::create(&zip_path).expect("zip");
    let mut writer = zip::ZipWriter::new(file);
    writer.start_file("nested/imported.json", zip::write::SimpleFileOptions::default()).expect("entry");
    writer
        .write_all(br#"{"info":{"name":"Zipped","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"item":[]}"#)
        .expect("write");
    writer.finish().expect("finish");
    let zipped = call(&automation, "import_zip", json!({"path": "incoming.zip"}));
    assert_eq!(zipped["items"][0]["outcome"]["name"], "Zipped");
    let zipped_id = zipped["items"][0]["outcome"]["id"].as_str().expect("zip collection id").to_string();
    call(&automation, "collection_delete", json!({"collection": zipped_id}));

    // Delete the collection; it lands in the trash with its item record intact.
    call(&automation, "collection_delete", json!({"collection": collection}));
    let info = call(&automation, "workspace_info", json!({}));
    assert_eq!(info["collections"], 0);
    let trash = dir.path().join("work/.trash");
    assert!(std::fs::read_dir(&trash).expect("trash dir").count() >= 1, "delete parked the trash metadata");
}

#[test]
fn oauth2_client_credentials_tool_updates_the_request_and_secret_store() {
    let server = postguy_testkit::Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    call(&automation, "workspace_open", json!({"path": "work"}));
    let collection = call(&automation, "collection_create", json!({"name": "OAuth"}))["collection"].as_str().unwrap().to_string();
    let request = call(&automation, "request_create", json!({"collection": collection, "kind": "http", "name": "Token"}))["request"]
        .as_str()
        .unwrap()
        .to_string();
    call(
        &automation,
        "request_update",
        json!({
            "collection": collection,
            "request": request,
            "definition": {
                "type": "http",
                "url": "http://example.test/",
                "auth": {
                    "kind": "oauth2",
                    "grantType": "clientCredentials",
                    "tokenUrl": format!("{}/token", server.base_url()),
                    "clientId": "client",
                    "clientSecret": "secret",
                    "clientAuthentication": "body"
                }
            }
        }),
    );
    let token = call(&automation, "oauth2_token", json!({"collection": collection, "request": request, "mode": "grant"}));
    assert_eq!(token["access_token"], "postguy-access-token");
    let got = call(&automation, "request_get", json!({"collection": collection, "request": request}));
    assert_eq!(got["request"]["kind"]["auth"]["accessToken"], "postguy-access-token");
    let file = std::fs::read_to_string(dir.path().join("work/collections").join(format!("{collection}.json"))).expect("collection file");
    assert!(!file.contains("postguy-access-token"), "the token lives in the secrets file: {file}");
    let secrets = std::fs::read_to_string(dir.path().join("work/.postguy/secrets.json")).expect("secrets");
    assert!(secrets.contains("postguy-access-token"));
}

#[test]
fn grpc_tools_describe_example_and_invoke_unary() {
    let server = postguy_testkit::GrpcEchoServer::start().expect("grpc server");
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    call(&automation, "workspace_open", json!({"path": "work"}));
    let proto_dir = dir.path().join("work/protos/echo");
    std::fs::create_dir_all(&proto_dir).expect("proto dir");
    std::fs::write(
        proto_dir.join("echo.proto"),
        r#"syntax = "proto3";
package echo;
service Echo {
  rpc Say (EchoMessage) returns (EchoMessage);
  rpc ServerStream (EchoMessage) returns (stream EchoMessage);
  rpc ClientStream (stream EchoMessage) returns (EchoMessage);
  rpc Bidi (stream EchoMessage) returns (stream EchoMessage);
}
message EchoMessage { string text = 1; int32 number = 2; }
"#,
    )
    .expect("proto");
    let collection = call(&automation, "collection_create", json!({"name": "gRPC"}))["collection"].as_str().unwrap().to_string();
    let request = call(&automation, "request_create", json!({"collection": collection, "kind": "grpc", "name": "Say"}))["request"]
        .as_str()
        .unwrap()
        .to_string();
    call(
        &automation,
        "request_update",
        json!({
            "collection": collection,
            "request": request,
            "definition": {
                "type": "grpc",
                "url": server.url(),
                "serviceDefinition": {"source": "protoSet", "name": "echo"},
                "methodPath": "echo.Echo/Say",
                "message": "{\"text\":\"hello\",\"number\":5}"
            }
        }),
    );
    let described = call(&automation, "grpc_describe", json!({"collection": collection, "request": request}));
    assert_eq!(described["services"][0]["methods"][0]["path"], "echo.Echo/Say");
    let example = call(&automation, "grpc_example_message", json!({"collection": collection, "request": request}));
    assert!(example["message"].is_object());
    let invoked = call(&automation, "grpc_invoke", json!({"collection": collection, "request": request}));
    assert_eq!(invoked["message"]["text"], "hello");
    assert_eq!(invoked["message"]["number"], 5);
    let failure = automation.call("grpc_invoke", &json!({"collection": collection, "request": request, "message": {"text": "fail"}}));
    assert!(failure.is_err(), "a status failure is reported: {failure:?}");
    // Reflection uses the same dynamic codec once the descriptor arrives from the server.
    call(
        &automation,
        "request_update",
        json!({
            "collection": collection,
            "request": request,
            "definition": {
                "type": "grpc",
                "url": server.url(),
                "serviceDefinition": {"source": "reflection"},
                "methodPath": "echo.Echo/Say",
                "message": "{\"text\":\"reflected\"}"
            }
        }),
    );
    let reflected = call(&automation, "grpc_invoke", json!({"collection": collection, "request": request}));
    assert_eq!(reflected["message"]["text"], "reflected");
    let update_method = |path: &str| {
        call(
            &automation,
            "request_update",
            json!({
                "collection": collection,
                "request": request,
                "definition": {
                    "type": "grpc",
                    "url": server.url(),
                    "serviceDefinition": {"source": "protoSet", "name": "echo"},
                    "methodPath": path,
                    "message": ""
                }
            }),
        );
    };
    // A stored request runs through the same scope/auth preparation as HTTP: a variable in the
    // URL resolves from the active environment and inherited bearer auth becomes metadata.
    let environment = call(&automation, "env_create", json!({"name": "grpc"}))["environment"].as_str().unwrap().to_string();
    call(&automation, "env_set_var", json!({"environment": environment, "key": "base", "value": server.url()}));
    call(&automation, "env_set_active", json!({"environment": environment}));
    call(
        &automation,
        "request_update",
        json!({
            "collection": collection,
            "request": request,
            "definition": {
                "type": "grpc",
                "url": "{{base}}",
                "serviceDefinition": {"source": "protoSet", "name": "echo"},
                "methodPath": "echo.Echo/Say",
                "message": "{\"text\":\"prepared\"}",
                "auth": {"kind": "bearer", "token": "inherited-token"}
            }
        }),
    );
    let prepared = call(&automation, "grpc_invoke", json!({"collection": collection, "request": request}));
    assert_eq!(prepared["message"]["text"], "prepared");
    assert!(
        prepared["metadata"].as_array().is_some_and(|rows| rows.iter().any(|row| row[0] == "authorization" && row[1] == "Bearer inherited-token")),
        "inherited auth is sent: {prepared}"
    );
    update_method("echo.Echo/ServerStream");
    let streamed = call(&automation, "grpc_stream", json!({"collection": collection, "request": request, "messages": [{"text": "s"}]}));
    assert_eq!(streamed["messages"].as_array().map(Vec::len), Some(3));
    assert_eq!(streamed["messages"][2]["number"], 3);
    update_method("echo.Echo/ClientStream");
    let collected =
        call(&automation, "grpc_stream", json!({"collection": collection, "request": request, "messages": [{"number": 4}, {"number": 6}]}));
    assert_eq!(collected["messages"].as_array().map(Vec::len), Some(1));
    assert_eq!(collected["messages"][0]["number"], 10);
    assert_eq!(collected["messages"][0]["text"], "2 messages, total 10");
    update_method("echo.Echo/Bidi");
    let bidi = call(&automation, "grpc_stream", json!({"collection": collection, "request": request, "messages": [{"text": "a"}, {"text": "b"}]}));
    assert_eq!(bidi["messages"].as_array().map(Vec::len), Some(2));
    assert_eq!(bidi["messages"][1]["text"], "b");
}

#[test]
fn grpc_proto_paths_cannot_escape_the_workspace() {
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    call(&automation, "workspace_open", json!({"path": "work"}));
    let collection = call(&automation, "collection_create", json!({"name": "gRPC"}))["collection"].as_str().unwrap().to_string();
    let request = call(&automation, "request_create", json!({"collection": collection, "kind": "grpc", "name": "Leak"}))["request"]
        .as_str()
        .unwrap()
        .to_string();
    let outside = dir.path().join("outside.proto");
    std::fs::write(&outside, "syntax = \"proto3\"; message Outside { string secret = 1; }").expect("outside proto");
    std::fs::create_dir_all(dir.path().join("work/protos/echo")).expect("proto dir");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, dir.path().join("work/protos/echo/leak.proto")).expect("symlink");
    let definition = |proto_path: &str, import_paths: Vec<&str>| {
        json!({
            "type": "grpc",
            "url": "http://127.0.0.1:1",
            "serviceDefinition": {
                "source": "inline",
                "protos": [{"name": "leak", "path": proto_path}],
                "importPaths": import_paths
            },
            "methodPath": "echo.Echo/Say",
            "message": "{}"
        })
    };
    let update = |definition: Value| {
        call(&automation, "request_update", json!({"collection": collection, "request": request, "definition": definition}));
    };
    for path in ["../../outside.proto", "../../missing.proto", &outside.to_string_lossy()] {
        update(definition(path, Vec::new()));
        assert!(
            automation.call("grpc_describe", &json!({"collection": collection, "request": request})).is_err(),
            "inline proto path `{path}` must refuse"
        );
    }
    update(definition("echo/leak.proto", Vec::new()));
    assert!(
        automation.call("grpc_describe", &json!({"collection": collection, "request": request})).is_err(),
        "a symlinked proto that leaves the protos root must refuse"
    );
    update(json!({
        "type": "grpc",
        "url": "http://127.0.0.1:1",
        "serviceDefinition": {"source": "protoSet", "name": "../.."},
        "methodPath": "echo.Echo/Say",
        "message": "{}"
    }));
    assert!(
        automation.call("grpc_describe", &json!({"collection": collection, "request": request})).is_err(),
        "a traversal proto set name must refuse"
    );
}

#[test]
fn websocket_tools_connect_echo_and_close() {
    let server = postguy_testkit::WsEchoServer::start().expect("ws server");
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    call(&automation, "workspace_open", json!({"path": "work"}));
    let collection = call(&automation, "collection_create", json!({"name": "WS"}))["collection"].as_str().unwrap().to_string();
    let request = call(&automation, "request_create", json!({"collection": collection, "kind": "websocket", "name": "Echo"}))["request"]
        .as_str()
        .unwrap()
        .to_string();
    call(
        &automation,
        "request_update",
        json!({"collection": collection, "request": request, "definition": {"type": "webSocket", "url": server.url()}}),
    );
    let session = call(&automation, "ws_connect", json!({"collection": collection, "request": request}))["session"].as_str().unwrap().to_string();
    let open = wait_ws(&automation, &session, 0, |events| events.iter().any(|event| event["kind"] == "open"));
    assert!(!open.is_empty());
    call(&automation, "ws_send", json!({"session": session, "text": "hello"}));
    let events = wait_ws(&automation, &session, 0, |events| events.iter().any(|event| event["direction"] == "received" && event["text"] == "hello"));
    assert!(events.iter().any(|event| event["direction"] == "sent" && event["text"] == "hello"));
    call(&automation, "ws_send", json!({"session": session, "binary_base64": "AAECAwQ="}));
    let binary = wait_ws(&automation, &session, 0, |events| events.iter().any(|event| event["direction"] == "received" && event["kind"] == "binary"));
    assert!(
        binary.iter().any(|event| event["direction"] == "sent" && event["kind"] == "binary" && event["dataBase64"] == "AAECAwQ="),
        "sent binary payload is retained: {binary:?}"
    );
    assert!(
        binary.iter().any(|event| event["direction"] == "received" && event["kind"] == "binary" && event["dataBase64"] == "AAECAwQ="),
        "received binary payload round-trips: {binary:?}"
    );
    call(&automation, "ws_disconnect", json!({"session": session, "code": 1000, "reason": "done"}));
}

fn wait_ws(automation: &Automation, session: &str, since: u64, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    for _ in 0..200 {
        let result = call(automation, "ws_messages", json!({"session": session, "since": since}));
        let events = result["events"].as_array().cloned().unwrap_or_default();
        if done(&events) {
            return events;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("WebSocket events did not arrive");
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
    let second = call(&automation, "import_text", json!({"text": collection_json}));
    assert_eq!(second["name"], "Imported (2)", "a name clash gets a suffix, never a silent overwrite");
    assert_ne!(second["id"], "11111111-2222-3333-4444-555555555555", "the clashing id gets a fresh id");
    let replaced = call(&automation, "import_text", json!({"text": collection_json, "replace": true}));
    assert_eq!(replaced["id"], "11111111-2222-3333-4444-555555555555");

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
