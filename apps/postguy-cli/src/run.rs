//! `postguy-cli run --collection ID_OR_NAME --workspace DIR`: open the workspace, resolve names
//! to ids, call the `collection_run` tool, print the reporters and exit 0 (all passed), 1
//! (a failure) or 2 (usage). Everything goes through the same tools an agent calls.

use std::path::PathBuf;

use postguy_automation::Automation;
use postguy_runner::RunResult;
use serde_json::{Value, json};

use crate::{automation, out, take_flag};

const USAGE: &str = "run needs either --script FILE or --collection ID --workspace DIR";

/// Remove a boolean switch; true when it was present.
fn take_switch(args: &[String], name: &str) -> (bool, Vec<String>) {
    let present = args.iter().any(|arg| arg == name);
    (present, args.iter().filter(|arg| *arg != name).cloned().collect())
}

/// An id, or a unique exact name, from `(id, name)` candidates.
fn resolve(kind: &str, wanted: &str, candidates: &[(String, String)]) -> Result<String, String> {
    if let Some((id, _)) = candidates.iter().find(|(id, _)| id == wanted) {
        return Ok(id.clone());
    }
    let named: Vec<&(String, String)> = candidates.iter().filter(|(_, name)| name == wanted).collect();
    match named.as_slice() {
        [(id, _)] => Ok(id.clone()),
        [] => {
            let known = candidates.iter().map(|(id, name)| format!("{name} ({id})")).collect::<Vec<_>>().join(", ");
            Err(format!("no {kind} `{wanted}`; known: {}", if known.is_empty() { "none".to_string() } else { known }))
        }
        many => {
            let ids = many.iter().map(|(id, name)| format!("{name} ({id})")).collect::<Vec<_>>().join(", ");
            Err(format!("`{wanted}` matches {} {kind}s; use an id: {ids}", many.len()))
        }
    }
}

fn pairs(value: &Value, key: &str) -> Vec<(String, String)> {
    value[key]
        .as_array()
        .map(|items| items.iter().filter_map(|item| Some((item["id"].as_str()?.to_string(), item["name"].as_str()?.to_string()))).collect())
        .unwrap_or_default()
}

/// Every folder of a collection JSON as `(id, name)`.
fn folders(collection: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack: Vec<&Value> = collection["collection"]["items"].as_array().map(|items| items.iter().collect()).unwrap_or_default();
    let mut visited = 0usize;
    while let Some(item) = stack.pop() {
        visited += 1;
        if visited > 200_000 {
            break;
        }
        if item["itemType"] == "folder" {
            let folder = &item["item"];
            if let (Some(id), Some(name)) = (folder["id"].as_str(), folder["name"].as_str()) {
                out.push((id.to_string(), name.to_string()));
            }
            if let Some(children) = folder["items"].as_array() {
                stack.extend(children.iter());
            }
        }
    }
    out
}

fn call(automation: &Automation, tool: &str, args: Value) -> Result<Value, String> {
    automation.call(tool, &args).map_err(|err| format!("{tool}: {err}"))
}

pub fn run(args: &[String]) -> i32 {
    let (collection, rest) = take_flag(args, "--collection");
    let (workspace, rest) = take_flag(&rest, "--workspace");
    let (folder, rest) = take_flag(&rest, "--folder");
    let (environment, rest) = take_flag(&rest, "--environment");
    let (iterations, rest) = take_flag(&rest, "--iterations");
    let (delay, rest) = take_flag(&rest, "--delay");
    let (data, rest) = take_flag(&rest, "--data");
    let (requests, rest) = take_flag(&rest, "--requests");
    let (reporters, rest) = take_flag(&rest, "--reporters");
    let (out_dir, rest) = take_flag(&rest, "--out");
    let (root, rest) = take_flag(&rest, "--root");
    let (bail, _) = take_switch(&rest, "--bail");

    let (Some(collection), Some(workspace)) = (collection, workspace) else {
        eprintln!("postguy-cli: {USAGE}");
        return 2;
    };
    let number = |name: &str, value: Option<String>| -> Result<Option<u64>, String> {
        value.map(|text| text.trim().parse::<u64>().map_err(|_| format!("{name} must be a whole number, got `{text}`"))).transpose()
    };
    let (iterations, delay) = match (number("--iterations", iterations), number("--delay", delay)) {
        (Ok(iterations), Ok(delay)) => (iterations, delay),
        (Err(err), _) | (_, Err(err)) => {
            eprintln!("postguy-cli run: {err}");
            return 2;
        }
    };
    let reporters: Vec<String> =
        reporters.unwrap_or_else(|| "cli".to_string()).split(',').map(|name| name.trim().to_string()).filter(|name| !name.is_empty()).collect();
    if let Some(bad) = reporters.iter().find(|name| !["cli", "json", "junit", "html"].contains(&name.as_str())) {
        eprintln!("postguy-cli run: unknown reporter `{bad}`; use cli, json, junit or html");
        return 2;
    }
    let automation = match automation(root.map(PathBuf::from)) {
        Ok(automation) => automation,
        Err(err) => {
            eprintln!("postguy-cli run: {err}");
            return 2;
        }
    };

    let prepared = (|| -> Result<(String, Value), String> {
        call(&automation, "workspace_open", json!({"path": workspace}))?;
        let listed = call(&automation, "collection_list", json!({}))?;
        let collection_id = resolve("collection", &collection, &pairs(&listed, "collections"))?;
        let mut args = json!({ "collection": collection_id, "stop_on_error": bail });
        if let Some(folder) = &folder {
            let full = call(&automation, "collection_get", json!({"collection": collection_id}))?;
            args["folder"] = json!(resolve("folder", folder, &folders(&full))?);
        }
        if let Some(environment) = &environment {
            let listed = call(&automation, "env_list", json!({}))?;
            args["environment"] = json!(resolve("environment", environment, &pairs(&listed, "environments"))?);
        }
        if let Some(iterations) = iterations {
            args["iterations"] = json!(iterations);
        }
        if let Some(delay) = delay {
            args["delay_ms"] = json!(delay);
        }
        if let Some(data) = &data {
            args["data"] = json!(data);
        }
        if let Some(requests) = &requests {
            args["requests"] = json!(requests.split(',').map(str::trim).filter(|id| !id.is_empty()).collect::<Vec<_>>());
        }
        Ok((collection_id, args))
    })();
    let (collection_id, run_args) = match prepared {
        Ok(prepared) => prepared,
        Err(err) => {
            eprintln!("postguy-cli run: {err}");
            // A name that matches nothing (or too much) is a usage problem; the rest is a failure.
            return if err.contains("no collection") || err.contains("no folder") || err.contains("no environment") || err.contains("matches") {
                2
            } else {
                1
            };
        }
    };
    let value = match call(&automation, "collection_run", run_args) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{}", json!({"ok": false, "error": err}));
            return 1;
        }
    };
    let result: RunResult = match serde_json::from_value(value) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("postguy-cli run: the run result is not readable: {err}");
            return 1;
        }
    };
    let dir = out_dir.unwrap_or_else(|| ".".to_string());
    for reporter in &reporters {
        match reporter.as_str() {
            "cli" => out(postguy_runner::report::cli(&result).trim_end()),
            format => {
                let extension = if format == "junit" { "xml" } else { format };
                let target = format!("{}/postguy-run-{}.{extension}", dir.trim_end_matches('/'), result.id);
                match call(&automation, "run_export", json!({"collection": collection_id, "run": result.id, "format": format, "out": target})) {
                    Ok(written) => out(format!("wrote {}", written["written"].as_str().unwrap_or(&target))),
                    Err(err) => {
                        eprintln!("postguy-cli run: {err}");
                        return 1;
                    }
                }
            }
        }
    }
    i32::from(!result.passed())
}
