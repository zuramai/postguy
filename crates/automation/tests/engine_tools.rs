//! End-to-end: the HTTP tools over a real engine and the loopback testkit — a stored
//! request sent through the pipeline, response retrieval/saving/rendering, cookies, history,
//! console and variable explanation, plus the cancellation contract.

use postguy_automation::{Automation, Root};
use postguy_testkit::Server;
use serde_json::{Value, json};

fn automation_in(dir: &std::path::Path) -> Automation {
    let root = Root::new(dir).expect("temp root canonicalizes");
    Automation::new(Some(root))
}

fn call(automation: &Automation, tool: &str, args: Value) -> Value {
    automation.call(tool, &args).unwrap_or_else(|err| panic!("{tool} failed: {err}"))
}

fn error_of(automation: &Automation, tool: &str, args: Value) -> String {
    automation.call(tool, &args).expect_err(tool).to_string()
}

/// Open a workspace, create a collection/request, point it at `url` and return their ids plus
/// the request id. The HTTP definition goes through `request_update`, the same path an agent uses.
fn prepared_request(automation: &Automation, url: &str) -> (String, String) {
    call(automation, "workspace_open", json!({"path": "work"}));
    create_request(automation, url)
}

/// The same, for a test that already opened the workspace once (the lock is per process).
fn create_request(automation: &Automation, url: &str) -> (String, String) {
    let collection =
        call(automation, "collection_create", json!({"name": "Engine tools"}))["collection"].as_str().expect("collection id").to_string();
    let request = call(automation, "request_create", json!({"collection": collection, "kind": "http", "name": "Echo"}))["request"]
        .as_str()
        .expect("request id")
        .to_string();
    let got = call(automation, "request_get", json!({"collection": collection, "request": request}));
    let mut definition = got["request"]["kind"].clone();
    definition["url"] = json!(url);
    definition["method"] = json!("GET");
    call(automation, "request_update", json!({"collection": collection, "request": request, "definition": definition}));
    (collection, request)
}

#[test]
fn request_send_and_the_response_tools_share_one_cached_outcome() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    let (collection, request) = prepared_request(&automation, &format!("{}/echo?tool=1", server.base_url()));

    let sent = call(&automation, "request_send", json!({"collection": collection, "request": request}));
    assert_eq!(sent["response"]["status"], 200, "{sent}");
    assert_eq!(sent["response"]["httpVersion"], "HTTP/1.1");
    assert!(sent["response"]["timings"]["totalMs"].as_u64().is_some());
    let history_id = sent["historyId"].as_str().expect("history id").to_string();
    let body: Value = serde_json::from_str(sent["response"]["bodyText"].as_str().expect("utf-8 body")).expect("echo json");
    assert_eq!(body["method"], "GET");
    assert_eq!(body["query"], "tool=1");

    // response_get addresses the same outcome by history id.
    let got = call(&automation, "response_get", json!({"id": history_id}));
    assert_eq!(got["response"]["status"], 200);
    assert_eq!(got["historyId"], history_id);
    let rendered = call(&automation, "response_render_body", json!({"id": history_id, "format": "text"}));
    assert!(rendered["body"].as_str().is_some_and(|body| body.contains("\"method\"")));

    // response_save writes the decoded body inside the root, atomically.
    let saved = call(&automation, "response_save", json!({"id": history_id, "path": "saved/echo.json"}));
    let saved_path = saved["written"].as_str().expect("written path");
    let bytes = std::fs::read(saved_path).expect("saved body");
    assert!(serde_json::from_slice::<Value>(&bytes).is_ok());

    // History and console recorded the send.
    let history = call(&automation, "history_list", json!({}));
    assert_eq!(history["entries"].as_array().map(Vec::len), Some(1));
    assert_eq!(history["entries"][0]["status"], 200);
    let reopened = call(&automation, "history_open", json!({"id": history_id}));
    assert_eq!(reopened["entry"]["id"], history_id);
    let console = call(&automation, "console_get", json!({"level": "info"}));
    assert!(console["entries"].as_array().is_some_and(|entries| entries.iter().any(|entry| entry["kind"] == "request")));

    // Clearing removes both views but leaves the response cache addressable.
    call(&automation, "history_clear", json!({}));
    call(&automation, "console_clear", json!({}));
    assert_eq!(call(&automation, "history_list", json!({}))["entries"].as_array().map(Vec::len), Some(0));
    assert_eq!(call(&automation, "console_get", json!({}))["entries"].as_array().map(Vec::len), Some(0));
    assert_eq!(call(&automation, "response_get", json!({"id": history_id}))["response"]["status"], 200);
}

#[test]
fn the_sent_body_reflects_pre_request_scripts_is_masked_and_stays_out_of_history() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    let (collection, request) = prepared_request(&automation, &format!("{}/echo", server.base_url()));
    let environment = call(&automation, "env_create", json!({"name": "Local"}))["environment"].as_str().expect("environment id").to_string();
    call(&automation, "env_set_var", json!({"environment": environment, "key": "token", "value": "hunter2", "secret": true}));

    let got = call(&automation, "request_get", json!({"collection": collection, "request": request}));
    let mut definition = got["request"]["kind"].clone();
    definition["method"] = json!("POST");
    definition["body"] = json!({"type": "raw", "language": "json", "text": "{\"authored\":true}"});
    // The payload is assembled by the script so its literal never appears in the stored script.
    definition["scripts"] = json!({
        "prerequest": {"text": "pm.request.body.update(JSON.stringify({image: 'aGVs' + 'bG8=', token: pm.variables.get('token')}));"}
    });
    call(&automation, "request_update", json!({"collection": collection, "request": request, "definition": definition}));

    let sent = call(&automation, "request_send", json!({"collection": collection, "request": request, "environment": environment}));
    let echo: Value = serde_json::from_str(sent["response"]["bodyText"].as_str().expect("utf-8 echo")).expect("echo json");
    assert!(echo["body"]["utf8"].as_str().is_some_and(|body| body.contains("hunter2")), "the server got the real secret: {echo}");
    let body_text = sent["request"]["bodyText"].as_str().expect("sent body text");
    assert_eq!(body_text, "{\"image\":\"aGVsbG8=\",\"token\":\"•••••••\"}", "the script's body, secret masked");
    assert_eq!(sent["request"]["bodyLen"], 38);
    assert_eq!(sent["request"]["bodyTruncated"], false);

    let history_id = sent["historyId"].as_str().expect("history id");
    assert_eq!(call(&automation, "response_get", json!({"id": history_id}))["request"]["bodyText"], body_text);
    let history_file = std::fs::read_to_string(dir.path().join("work/history.jsonl")).expect("history file");
    assert!(!history_file.contains("aGVsbG8="), "history.jsonl never stores the body: {history_file}");
}

#[test]
fn cookies_are_captured_listed_deleted_and_cleared() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    let (collection, request) = prepared_request(&automation, &format!("{}/cookies/set?jar=one", server.base_url()));
    call(&automation, "request_send", json!({"collection": collection, "request": request}));

    let listed = call(&automation, "cookies_list", json!({}));
    let cookies = listed["cookies"].as_array().expect("cookies");
    assert!(cookies.iter().any(|cookie| cookie["name"] == "jar" && cookie["value"] == "one"), "{listed}");
    let domain = cookies[0]["domain"].as_str().expect("domain").to_string();

    call(&automation, "cookies_set", json!({"url": format!("{}/x", server.base_url()), "setCookie": "manual=2; Path=/x"}));
    let listed = call(&automation, "cookies_list", json!({}));
    assert!(listed["cookies"].as_array().is_some_and(|cookies| cookies.iter().any(|cookie| cookie["name"] == "manual")));

    call(&automation, "cookies_delete", json!({"domain": domain, "path": "/", "name": "jar"}));
    let listed = call(&automation, "cookies_list", json!({}));
    assert!(listed["cookies"].as_array().is_some_and(|cookies| cookies.iter().all(|cookie| cookie["name"] != "jar")));

    call(&automation, "cookies_clear", json!({}));
    assert_eq!(call(&automation, "cookies_list", json!({}))["cookies"].as_array().map(Vec::len), Some(0));
}

#[test]
fn variables_resolve_names_the_scope_and_secret_origin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    call(&automation, "workspace_open", json!({"path": "work"}));
    let environment = call(&automation, "env_create", json!({"name": "Local"}))["environment"].as_str().expect("environment id").to_string();
    call(&automation, "env_set_var", json!({"environment": environment, "key": "base", "value": "http://127.0.0.1:9"}));
    call(&automation, "env_set_var", json!({"environment": environment, "key": "token", "value": "hunter2", "secret": true}));

    let resolved = call(&automation, "variables_resolve", json!({"text": "{{base}}/x?key={{token}}&missing={{nope}}", "environment": environment}));
    assert_eq!(resolved["text"], "http://127.0.0.1:9/x?key=hunter2&missing={{nope}}");
    let hits = resolved["hits"].as_array().expect("hits");
    assert!(hits.iter().any(|hit| hit["name"] == "base" && hit["source"] == "environment" && hit["unresolved"] == false));
    assert!(hits.iter().any(|hit| hit["name"] == "token" && hit["secret"] == true));
    assert!(hits.iter().any(|hit| hit["name"] == "nope" && hit["unresolved"] == true));
}

/// The full send path through the tool layer: redirect loops, compressed responses, a self-signed TLS
/// server (verify off is an explicit per-request setting), the response cap and cancellation.
#[test]
fn the_send_path_covers_redirects_compression_tls_limits_and_cancel() {
    let server = Server::start();
    let tls = postguy_testkit::Server::start_tls();
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    call(&automation, "workspace_open", json!({"path": "work"}));

    // Redirect loop: an actionable error, not a hang.
    let (collection, request) = create_request(&automation, &format!("{}/redirect/loop", server.base_url()));
    let message = error_of(&automation, "request_send", json!({"collection": collection, "request": request}));
    assert!(message.contains("redirect loop"), "{message}");

    // Compression: gzip is negotiated and decoded.
    let (collection, request) = create_request(&automation, &format!("{}/compressed?kind=gzip&size=4096", server.base_url()));
    let sent = call(&automation, "request_send", json!({"collection": collection, "request": request}));
    assert_eq!(sent["response"]["status"], 200);
    assert_eq!(sent["response"]["bodyLength"], 4096, "decoded size: {sent}");
    assert_eq!(sent["response"]["size"]["contentEncoding"], "gzip");

    // Self-signed TLS: verification fails by default with a TLS phase message.
    let (collection, request) = create_request(&automation, &format!("{}/echo", tls.base_url()));
    let message = error_of(&automation, "request_send", json!({"collection": collection, "request": request}));
    assert!(message.contains("TLS"), "{message}");

    // ...and succeeds when the per-request verify toggle is off (shown as a warning chip in UI).
    let got = call(&automation, "request_get", json!({"collection": collection, "request": request}));
    let mut definition = got["request"]["kind"].clone();
    definition["settings"]["verifySsl"] = json!(false);
    definition["url"] = json!(format!("{}/echo", tls.base_url()));
    call(&automation, "request_update", json!({"collection": collection, "request": request, "definition": definition}));
    let sent = call(&automation, "request_send", json!({"collection": collection, "request": request}));
    assert_eq!(sent["response"]["status"], 200, "verify off: {sent}");

    // A response over the per-request size cap errors with the cap named.
    let (collection, request) = create_request(&automation, &format!("{}/huge?mb=2", server.base_url()));
    let got = call(&automation, "request_get", json!({"collection": collection, "request": request}));
    let mut definition = got["request"]["kind"].clone();
    definition["settings"]["maxResponseMb"] = json!(1);
    definition["url"] = json!(format!("{}/huge?mb=2", server.base_url()));
    call(&automation, "request_update", json!({"collection": collection, "request": request, "definition": definition}));
    let message = error_of(&automation, "request_send", json!({"collection": collection, "request": request}));
    assert!(message.contains("cap"), "{message}");

    // Cancellation reaches a send that is in flight in another thread.
    let (collection, request) = create_request(&automation, &format!("{}/slow?ms=3000", server.base_url()));
    let automation = std::sync::Arc::new(automation);
    let worker = {
        let automation = automation.clone();
        let collection = collection.clone();
        let request = request.clone();
        std::thread::spawn(move || automation.call("request_send", &json!({"collection": collection, "request": request})))
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    let cancelled = call(&automation, "request_cancel", json!({"sendId": "last"}));
    assert_eq!(cancelled["cancelled"], true);
    let result = worker.join().expect("worker");
    let message = result.expect_err("the send was cancelled").to_string();
    assert!(message.contains("cancelled"), "{message}");
}

#[test]
fn cancelling_a_send_that_is_not_in_flight_explains_the_contract() {
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    call(&automation, "workspace_open", json!({"path": "work"}));
    let message = error_of(&automation, "request_cancel", json!({"sendId": "none"}));
    assert!(message.contains("no in-flight send"), "{message}");
    assert!(message.contains("last"), "{message}");
}

#[test]
fn request_send_reports_a_bad_request_as_an_actionable_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    let (collection, request) = prepared_request(&automation, "{{missing}}/x");
    let message = error_of(&automation, "request_send", json!({"collection": collection, "request": request}));
    assert!(message.contains("unresolved variable in its host"), "{message}");
}

#[test]
fn request_send_reports_script_tests() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    let (collection, request) = prepared_request(&automation, &format!("{}/echo", server.base_url()));
    let got = call(&automation, "request_get", json!({"collection": collection, "request": request}));
    let mut definition = got["request"]["kind"].clone();
    definition["scripts"] = json!({
        "test": {"text": "pm.test('status is 200', () => pm.response.to.have.status(200));\npm.test('fails', () => pm.expect(1).to.equal(2));\npm.environment.set('x', '1');"}
    });
    call(&automation, "request_update", json!({"collection": collection, "request": request, "definition": definition}));

    let sent = call(&automation, "request_send", json!({"collection": collection, "request": request}));
    assert_eq!(sent["script"]["tests"][0]["status"], "passed", "{sent}");
    assert_eq!(sent["script"]["tests"][1]["status"], "failed");
    assert_eq!(sent["script"]["tests"][1]["message"], "expected 1 to equal 2");
    assert_eq!(sent["script"]["tests"][0]["phase"], "postResponse");
    // No environment is active, so the write is reported as a script failure, not a crash.
    assert!(sent["script"]["failures"][0]["message"].as_str().is_some_and(|m| m.contains("no environment is selected")), "{sent}");
    // The same report comes back from the response tools.
    let again = call(&automation, "response_get", json!({}));
    assert_eq!(again["script"]["tests"][0]["name"], "status is 200");
}

/// A collection with one scripted GET `/echo/{{id}}` request; returns the collection id.
fn scripted_collection(automation: &Automation, base: &str, test: &str) -> (String, String) {
    let (collection, request) = prepared_request(automation, &format!("{base}/echo/{{{{id}}}}"));
    let got = call(automation, "request_get", json!({"collection": collection, "request": request}));
    let mut definition = got["request"]["kind"].clone();
    definition["scripts"] = json!({ "test": {"text": test} });
    call(automation, "request_update", json!({"collection": collection, "request": request, "definition": definition}));
    (collection, request)
}

#[test]
fn collection_run_runs_lists_gets_and_exports() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    let (collection, request) = scripted_collection(
        &automation,
        &server.base_url(),
        "pm.test('row ' + pm.iterationData.get('id'), () => pm.expect(pm.response.json().path).to.equal('/echo/' + pm.iterationData.get('id')));",
    );
    std::fs::write(dir.path().join("rows.csv"), "id\n7\n8\n").expect("data file");

    let result = call(&automation, "collection_run", json!({"collection": collection, "data": "rows.csv"}));
    assert_eq!(result["iterations"].as_array().map(Vec::len), Some(2), "{result}");
    assert_eq!(result["totals"]["testsPassed"], 2);
    assert_eq!(result["iterations"][1]["requests"][0]["tests"][0]["name"], "row 8");
    assert!(result["stopped"].is_null());
    let run_id = result["id"].as_str().expect("run id").to_string();

    let listed = call(&automation, "run_list", json!({"collection": collection}));
    assert_eq!(listed["runs"][0]["id"], run_id.as_str());
    assert_eq!(listed["runs"][0]["passed"], true);
    let got = call(&automation, "run_get", json!({"collection": collection, "run": run_id}));
    assert_eq!(got["totals"], result["totals"]);

    for (format, needle) in [("junit", "<testsuites"), ("html", "<!doctype html>"), ("json", "\"testsPassed\"")] {
        let out = format!("reports/run.{format}");
        let written = call(&automation, "run_export", json!({"collection": collection, "run": run_id, "format": format, "out": out}));
        assert_eq!(written["format"], format);
        let text = std::fs::read_to_string(dir.path().join(&out)).expect("report file");
        assert!(text.contains(needle), "{format}: {text}");
    }
    for (format, magic) in [("csv", &b"\xEF\xBB\xBF"[..]), ("xlsx", &b"PK"[..]), ("pdf", &b"%PDF-1.4"[..])] {
        let out = format!("reports/run.{format}");
        let written = call(&automation, "run_export", json!({"collection": collection, "run": run_id, "format": format, "out": out}));
        assert_eq!(written["format"], format);
        let bytes = std::fs::read(dir.path().join(&out)).expect("report file");
        assert!(bytes.starts_with(magic), "{format}: {:?}", &bytes[..bytes.len().min(8)]);
    }
    let message = error_of(&automation, "run_export", json!({"collection": collection, "run": run_id, "format": "docx", "out": "x.docx"}));
    assert!(message.contains("json, junit, html, csv, xlsx or pdf"), "{message}");
    let message = error_of(&automation, "run_get", json!({"collection": collection, "run": "../x"}));
    assert!(message.contains("run id"), "{message}");

    // A failing test makes the run not pass, and one request's id narrows a run to it.
    let got = call(&automation, "request_get", json!({"collection": collection, "request": request}));
    let mut definition = got["request"]["kind"].clone();
    definition["scripts"] = json!({"test": {"text": "pm.test('bad', () => pm.expect(1).to.equal(2));"}});
    definition["url"] = json!(format!("{}/echo/1", server.base_url()));
    call(&automation, "request_update", json!({"collection": collection, "request": request, "definition": definition}));
    let failed =
        call(&automation, "collection_run", json!({"collection": collection, "requests": [request], "iterations": 1, "stop_on_error": true}));
    assert_eq!(failed["totals"]["testsFailed"], 1);
    assert!(failed["stopped"].as_str().is_some_and(|s| s.contains("stop on error")), "{failed}");
    let message = error_of(&automation, "collection_run", json!({"collection": collection, "requests": "not-an-array"}));
    assert!(message.contains("an array of request ids"), "{message}");
}

#[test]
fn collection_run_data_paths_are_confined() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("allowed");
    std::fs::create_dir_all(&root).expect("root dir");
    let automation = Automation::new(Some(Root::new(&root).expect("canonical root")));
    call(&automation, "workspace_open", json!({"path": "work"}));
    let collection = call(&automation, "collection_create", json!({"name": "c"}))["collection"].as_str().expect("id").to_string();
    std::fs::write(dir.path().join("outside.csv"), "a\n1\n").expect("outside file");
    let missing = error_of(&automation, "collection_run", json!({"collection": collection, "data": "../ghost.csv"}));
    let existing = error_of(&automation, "collection_run", json!({"collection": collection, "data": "../outside.csv"}));
    assert!(missing.contains("is outside the allowed directory"), "{missing}");
    assert_eq!(missing.replace("../ghost.csv", "PATH"), existing.replace("../outside.csv", "PATH"));
    let out = error_of(&automation, "run_export", json!({"collection": collection, "run": "r", "format": "json", "out": "../escape.json"}));
    assert!(out.contains("is outside the allowed directory") || out.contains("run"), "{out}");
}

#[test]
fn asset_tools_import_list_rename_delete_and_feed_body_shortcodes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    let server = Server::start();
    let (collection, request) = prepared_request(&automation, &format!("{}/echo", server.base_url()));
    std::fs::create_dir_all(dir.path().join("pics")).expect("pics");
    std::fs::write(dir.path().join("pics/a.bin"), [0xffu8, 0x00, 0x10]).expect("asset");
    std::fs::write(dir.path().join("note.txt"), "hello").expect("note");

    let none = call(&automation, "asset_list", json!({"collection": collection}));
    assert_eq!(none, json!([]));
    let imported = call(&automation, "asset_import", json!({"collection": collection, "paths": ["pics", "note.txt"], "dir": "up"}));
    assert_eq!(imported["skipped"], json!([]), "{imported}");
    let listed = call(&automation, "asset_list", json!({"collection": collection}));
    let names: Vec<&str> = listed.as_array().expect("array").iter().filter_map(|a| a["path"].as_str()).collect();
    assert_eq!(names, ["up/note.txt", "up/pics/a.bin"]);
    assert_eq!(listed[0]["size"], 5);

    // A stored request uses an asset through a body shortcode.
    let got = call(&automation, "request_get", json!({"collection": collection, "request": request}));
    let mut definition = got["request"]["kind"].clone();
    definition["method"] = json!("POST");
    definition["body"] = json!({"type": "raw", "language": "text", "text": "{{% base64(file('up/pics/a.bin')) %}}"});
    call(&automation, "request_update", json!({"collection": collection, "request": request, "definition": definition}));
    let sent = call(&automation, "request_send", json!({"collection": collection, "request": request}));
    assert!(sent.to_string().contains("/wAQ"), "{sent}");

    let moved = call(&automation, "asset_rename", json!({"collection": collection, "from": "up/note.txt", "to": "notes/n.txt"}));
    assert_eq!(moved["path"], "notes/n.txt");
    let message = error_of(&automation, "asset_rename", json!({"collection": collection, "from": "notes/n.txt", "to": "up/pics/a.bin"}));
    assert!(message.contains("already exists"), "{message}");
    let deleted = call(&automation, "asset_delete", json!({"collection": collection, "path": "notes/n.txt"}));
    assert_eq!(deleted["deleted"], "notes/n.txt");
    let message = error_of(&automation, "asset_delete", json!({"collection": collection, "path": "../x"}));
    assert!(message.contains(".."), "{message}");
    let listed = call(&automation, "asset_list", json!({"collection": collection}));
    assert_eq!(listed.as_array().map(Vec::len), Some(1));
    let message = error_of(&automation, "asset_import", json!({"collection": collection}));
    assert!(message.contains("paths"), "{message}");
}

#[test]
fn asset_import_paths_are_confined_to_the_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("allowed");
    std::fs::create_dir_all(&root).expect("root dir");
    let automation = Automation::new(Some(Root::new(&root).expect("canonical root")));
    call(&automation, "workspace_open", json!({"path": "work"}));
    let collection = call(&automation, "collection_create", json!({"name": "c"}))["collection"].as_str().expect("id").to_string();
    std::fs::write(dir.path().join("outside.png"), "x").expect("outside file");
    let missing = error_of(&automation, "asset_import", json!({"collection": collection, "paths": ["../ghost.png"]}));
    let existing = error_of(&automation, "asset_import", json!({"collection": collection, "paths": ["../outside.png"]}));
    assert!(missing.contains("is outside the allowed directory"), "{missing}");
    assert_eq!(missing.replace("../ghost.png", "PATH"), existing.replace("../outside.png", "PATH"));
}

#[test]
fn runtime_variables_tool_reads_and_clears() {
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    let server = Server::start();
    let (collection, request) = prepared_request(&automation, &format!("{}/echo", server.base_url()));
    assert_eq!(call(&automation, "runtime_variables", json!({"collection": collection})), json!({}));
    let got = call(&automation, "request_get", json!({"collection": collection, "request": request}));
    let mut definition = got["request"]["kind"].clone();
    definition["scripts"] = json!({"test": {"text": "pg.setVar('session', 's-1');"}});
    call(&automation, "request_update", json!({"collection": collection, "request": request, "definition": definition}));
    call(&automation, "request_send", json!({"collection": collection, "request": request}));
    let held = call(&automation, "runtime_variables", json!({"collection": collection, "clear": true}));
    assert_eq!(held, json!({"session": "s-1"}));
    assert_eq!(call(&automation, "runtime_variables", json!({"collection": collection})), json!({}));
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read_dir") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy");
        }
    }
}

/// The id of the first object named `name` anywhere in `value`.
fn find_named_id(value: &Value, name: &str) -> Option<String> {
    match value {
        Value::Object(map) => {
            if map.get("name").and_then(Value::as_str) == Some(name)
                && let Some(id) = map.get("id").and_then(Value::as_str)
            {
                return Some(id.to_string());
            }
            map.values().find_map(|child| find_named_id(child, name))
        }
        Value::Array(items) => items.iter().find_map(|child| find_named_id(child, name)),
        _ => None,
    }
}

#[test]
fn import_bruno_runs_asset_backed_scripts_end_to_end() {
    use base64::Engine as _;
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let automation = automation_in(dir.path());
    call(&automation, "workspace_open", json!({"path": "work"}));
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../interop/tests/fixtures/bruno/sample");
    let source = dir.path().join("bruno-sample");
    copy_tree(&fixture, &source);
    let png = std::fs::read(source.join("lib/images/a.png")).expect("png");
    let expected = base64::engine::general_purpose::STANDARD.encode(&png);
    std::fs::create_dir_all(source.join("e2e")).expect("mkdir");
    std::fs::write(
        source.join("e2e/echo.bru"),
        format!(
            "meta {{\n  name: Echo image\n  type: http\n  seq: 1\n}}\n\npost {{\n  url: {{{{baseUrl}}}}/echo\n  body: json\n  auth: none\n}}\n\nbody:json {{\n  {{\"image\": \"{{{{img}}}}\"}}\n}}\n\nassert {{\n  res.status: eq 200\n  res.body.body.utf8: contains {expected}\n}}\n\nscript:pre-request {{\n  const utils = require('./lib/utils');\n  bru.setVar('img', utils.image('a.png'));\n}}\n"
        ),
    )
    .expect("echo request");

    let imported = call(&automation, "import_bruno", json!({"path": "bruno-sample"}));
    let collection = imported["collection"]["id"].as_str().expect("collection id").to_string();
    let environment = imported["environments"][0]["id"].as_str().expect("environment id").to_string();
    let assets: Vec<&str> = imported["assets"]["imported"].as_array().expect("assets").iter().filter_map(|a| a["path"].as_str()).collect();
    assert!(assets.contains(&"lib/utils.js") && assets.contains(&"lib/images/a.png"), "{assets:?}");
    call(&automation, "env_set_var", json!({"environment": environment, "key": "baseUrl", "value": server.base_url()}));

    let tree = call(&automation, "collection_get", json!({"collection": collection}));
    let request = find_named_id(&tree, "Echo image").expect("imported request id");
    let result =
        call(&automation, "collection_run", json!({"collection": collection, "environment": environment, "requests": [request], "iterations": 1}));
    let row = &result["iterations"][0]["requests"][0];
    assert_eq!(row["status"], 200, "{result}");
    assert!(row["error"].is_null(), "{result}");
    assert_eq!(result["totals"]["testsFailed"], 0, "{result}");
    assert!(result["totals"]["testsPassed"].as_u64().is_some_and(|n| n >= 2), "{result}");
}

#[test]
fn import_bruno_is_confined_to_the_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).expect("root");
    let automation = automation_in(&root);
    std::fs::create_dir_all(dir.path().join("outside")).expect("outside");
    let missing = error_of(&automation, "import_bruno", json!({"path": "../ghost"}));
    let existing = error_of(&automation, "import_bruno", json!({"path": "../outside"}));
    assert!(missing.contains("is outside the allowed directory"), "{missing}");
    assert!(existing.contains("is outside the allowed directory"), "{existing}");
}
