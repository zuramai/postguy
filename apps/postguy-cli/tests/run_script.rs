//! End-to-end through the REAL binary: `tools`, `call`, and `run --script --root` including
//! the confinement refusal and the exit codes an agent branches on.

use std::process::Command;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_postguy-cli"))
}

fn write_script(dir: &std::path::Path, name: &str, value: serde_json::Value) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, value.to_string()).expect("script file");
    path
}

#[test]
fn tools_lists_the_table_with_schemas() {
    let output = cli().arg("tools").arg("--json").output().expect("run tools");
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json table");
    let names: Vec<&str> = parsed["tools"].as_array().expect("array").iter().filter_map(|tool| tool["name"].as_str()).collect();
    for required in ["workspace_open", "collection_create", "request_create", "env_set_var", "import_text", "export_collection"] {
        assert!(names.contains(&required), "missing tool {required}: {names:?}");
    }
    // Every tool carries a schema with additionalProperties false.
    for tool in parsed["tools"].as_array().expect("array") {
        assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{}", tool["name"]);
    }
}

#[test]
fn call_runs_one_tool_and_reports_errors_on_stderr() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = cli().args(["call", "workspace_open", "--json", r#"{"path":"work"}"#]).arg("--root").arg(dir.path()).output().expect("run call");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json result");
    assert_eq!(parsed["collections"], 0);

    let bad = cli().args(["call", "collection_frobnicate", "--json", "{}"]).arg("--root").arg(dir.path()).output().expect("run bad call");
    assert_eq!(bad.status.code(), Some(1), "tool errors exit 1");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unknown tool"));
}

#[test]
fn run_script_drives_the_engine_and_stops_on_the_first_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(
        dir.path(),
        "steps.json",
        serde_json::json!([
            {"tool": "workspace_open", "args": {"path": "work"}},
            {"tool": "collection_create", "args": {"name": "Smoke"}},
            {"tool": "collection_list", "args": {}}
        ]),
    );
    let output = cli().args(["run", "--script"]).arg(&script).arg("--root").arg(dir.path()).output().expect("run script");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""tool":"collection_create""#), "{stdout}");
    assert!(stdout.contains(r#""collections":[{"#), "{stdout}");
    assert!(stdout.contains(r#""ok":true,"steps":3}"#), "{stdout}");

    // A failing step exits 1 and stops the run.
    let failing = write_script(
        dir.path(),
        "failing.json",
        serde_json::json!([
            {"tool": "workspace_open", "args": {"path": "work"}},
            {"tool": "collection_create", "args": {}},
            {"tool": "collection_list", "args": {}}
        ]),
    );
    let output = cli().args(["run", "--script"]).arg(&failing).arg("--root").arg(dir.path()).output().expect("run failing");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing required argument `name`"), "{stderr}");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("collection_list"), "stopped at the failing step");
}

#[test]
fn an_outside_script_is_refused_with_the_same_message_shape() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).expect("root");
    let outside = write_script(dir.path(), "outside.json", serde_json::json!([]));
    let output = cli().args(["run", "--script"]).arg(&outside).arg("--root").arg(&root).output().expect("run outside");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("is outside the allowed directory"), "{stderr}");
}

/// The REAL binary sends a stored request to the loopback testkit through
/// `postguy-cli run --script --root`, with the response tools reading the same outcome.
#[test]
fn run_script_sends_through_the_engine_to_loopback() {
    let server = postguy_testkit::Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // Each `run --script` is one process, so an agent chains steps inside one script; the
    // helper returns each step's result object.
    let mut counter = 0usize;
    let mut run_steps = |steps: serde_json::Value| -> Vec<serde_json::Value> {
        counter += 1;
        let script = write_script(dir.path(), &format!("steps-{counter}.json"), steps);
        let output = cli().args(["run", "--script"]).arg(&script).arg("--root").arg(root).output().expect("run script");
        assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|line| line["ok"] == true && line.get("result").is_some())
            .map(|line| line["result"].clone())
            .collect()
    };

    let created = run_steps(serde_json::json!([
        {"tool": "workspace_open", "args": {"path": "work"}},
        {"tool": "collection_create", "args": {"name": "CLI smoke"}}
    ]));
    let collection = created.last().expect("a result")["collection"].as_str().expect("collection id").to_string();

    let created = run_steps(serde_json::json!([
        {"tool": "workspace_open", "args": {"path": "work"}},
        {"tool": "request_create", "args": {"collection": collection, "kind": "http", "name": "Echo"}}
    ]));
    let request = created.last().expect("a result")["request"].as_str().expect("request id").to_string();

    let got = run_steps(serde_json::json!([
        {"tool": "workspace_open", "args": {"path": "work"}},
        {"tool": "request_get", "args": {"collection": collection, "request": request}}
    ]));
    let mut definition = got.last().expect("a result")["request"]["kind"].clone();
    definition["url"] = serde_json::json!(format!("{}/echo?cli=1", server.base_url()));
    definition["method"] = serde_json::json!("GET");
    run_steps(serde_json::json!([
        {"tool": "workspace_open", "args": {"path": "work"}},
        {"tool": "request_update", "args": {"collection": collection, "request": request, "definition": definition}}
    ]));

    let script = write_script(
        dir.path(),
        "send.json",
        serde_json::json!([
            {"tool": "workspace_open", "args": {"path": "work"}},
            {"tool": "request_send", "args": {"collection": collection, "request": request}},
            {"tool": "response_get", "args": {}},
            {"tool": "history_list", "args": {"limit": 5}}
        ]),
    );
    let output = cli().args(["run", "--script"]).arg(&script).arg("--root").arg(root).output().expect("run script");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""status":200"#), "{stdout}");
    assert!(stdout.contains(r#"cli=1"#), "the echo body came back: {stdout}");
    assert!(stdout.contains(r#""historyId""#), "{stdout}");
    assert!(stdout.contains(r#""ok":true,"steps":4}"#), "{stdout}");
}
