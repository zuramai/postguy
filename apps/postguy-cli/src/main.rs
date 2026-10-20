//! Postguy headless CLI: the automation tool table (`tools`, `call`, `run --script`) and the
//! `fuzz-target` dev verb (PLAN M1.9), and the `ui` client for the app's opt-in control
//! channel (M3.10). MCP arrives with its milestone.
//!
//! `--root DIR` confines every path argument: outside paths get the SAME refusal whether or
//! not they exist (AGENTS.md 3).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod fuzz_targets;
mod ui;

use std::io::Write as _;
use std::path::PathBuf;

use postguy_automation::{Automation, Root, tools};
use serde_json::Value;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("tools") => run_tools(&args[1..]),
        Some("call") => run_call(&args[1..]),
        Some("run") => run_script(&args[1..]),
        Some("fuzz-target") => run_fuzz(&args[1..]),
        Some("ui") => ui::run(&args[1..]),
        _ => {
            usage();
            2
        }
    };
    std::process::exit(code);
}

/// Print one line to stdout, tolerating a closed pipe (piping into `head` is normal use).
fn out(line: impl std::fmt::Display) {
    let _ = writeln!(std::io::stdout(), "{line}");
}

fn usage() {
    eprintln!(
        "usage:\n  postguy-cli tools [--json]\n  postguy-cli call <tool> --json '<args>' [--root DIR]\n  postguy-cli run --script FILE [--root DIR]\n  postguy-cli fuzz-target <name> <seed-file>   (dev tooling)\n  postguy-cli ui --control FILE inspect|command ID|set KEY=VALUE|screenshot --out FILE|click X Y|key NAME|type TEXT [--root DIR]\n\nRun `postguy-cli tools` for the tool table; `ui` talks to an app launched with `--control FILE`."
    );
}

fn automation(root: Option<PathBuf>) -> Result<Automation, String> {
    let root = match root {
        Some(dir) => {
            std::fs::create_dir_all(&dir).map_err(|err| format!("creating --root {}: {err}", dir.display()))?;
            Some(Root::new(&dir).map_err(|err| err.to_string())?)
        }
        None => None,
    };
    Ok(Automation::new(root))
}

fn take_flag(args: &[String], name: &str) -> (Option<String>, Vec<String>) {
    let mut value = None;
    let mut rest = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == name {
            if let Some(next) = args.get(index + 1) {
                value = Some(next.clone());
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        rest.push(args[index].clone());
        index += 1;
    }
    (value, rest)
}

fn run_tools(args: &[String]) -> i32 {
    let json_out = args.iter().any(|arg| arg == "--json");
    if json_out {
        let table: Vec<Value> = tools()
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "title": tool.title,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                    "readOnly": tool.read_only,
                    "destructive": tool.destructive,
                })
            })
            .collect();
        out(serde_json::json!({"tools": table}));
        return 0;
    }
    for tool in tools() {
        let safety = match (tool.read_only, tool.destructive) {
            (true, _) => "read-only",
            (false, true) => "destructive",
            _ => "write",
        };
        out(format!("{:<20} {:<12} {}", tool.name, safety, tool.title));
    }
    0
}

fn run_call(args: &[String]) -> i32 {
    let (json_args, without_json) = take_flag(args, "--json");
    let (root, rest) = take_flag(&without_json, "--root");
    let Some(tool) = rest.first() else {
        eprintln!("postguy-cli call: missing tool name");
        usage();
        return 2;
    };
    let parsed: Value = match json_args {
        Some(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(err) => {
                eprintln!("postguy-cli call: --json is not valid JSON: {err}");
                return 2;
            }
        },
        None => Value::Null,
    };
    match automation(root.map(PathBuf::from)) {
        Err(err) => {
            eprintln!("postguy-cli call: {err}");
            2
        }
        Ok(automation) => match automation.call(tool, &parsed) {
            Ok(result) => {
                out(result);
                0
            }
            Err(err) => {
                eprintln!("{}", serde_json::json!({"ok": false, "error": err.to_string()}));
                1
            }
        },
    }
}

fn run_script(args: &[String]) -> i32 {
    let (script, without_script) = take_flag(args, "--script");
    let (root, _) = take_flag(&without_script, "--root");
    let Some(script) = script else {
        eprintln!("postguy-cli run: --script FILE is required");
        usage();
        return 2;
    };
    let automation = match automation(root.map(PathBuf::from)) {
        Ok(automation) => automation,
        Err(err) => {
            eprintln!("postguy-cli run: {err}");
            return 2;
        }
    };
    // The script path itself resolves through the same confinement as tool paths (a root
    // refuses an outside script before reading it).
    let path = match script_path(&automation, &script) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("postguy-cli run: {err}");
            return 2;
        }
    };
    let bytes = match postguy_util::read_file_capped(&path, 8 << 20) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("postguy-cli run: reading {}: {err}", path.display());
            return 2;
        }
    };
    let parsed: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("postguy-cli run: {} is not JSON: {err}", path.display());
            return 2;
        }
    };
    let steps = match &parsed {
        Value::Array(steps) => steps.clone(),
        Value::Object(map) => match map.get("steps") {
            Some(Value::Array(steps)) => steps.clone(),
            _ => {
                eprintln!("postguy-cli run: script needs a `steps` array");
                return 2;
            }
        },
        _ => {
            eprintln!("postguy-cli run: script must be an array of steps or an object with a steps array");
            return 2;
        }
    };
    let mut executed = 0usize;
    for (index, step) in steps.iter().enumerate() {
        let tool = step.get("tool").and_then(Value::as_str).unwrap_or_default();
        if tool.is_empty() {
            eprintln!("postguy-cli run: step {index} has no `tool`");
            return 1;
        }
        let step_args = step.get("args").cloned().unwrap_or(Value::Null);
        match automation.call(tool, &step_args) {
            Ok(result) => {
                out(serde_json::json!({"step": index, "tool": tool, "ok": true, "result": result}));
                executed += 1;
            }
            Err(err) => {
                eprintln!("{}", serde_json::json!({"step": index, "tool": tool, "ok": false, "error": err.to_string()}));
                return 1;
            }
        }
    }
    out(serde_json::json!({"ok": true, "steps": executed}));
    0
}

/// Resolve the script path through the automation root when one is configured.
fn script_path(automation: &Automation, script: &str) -> Result<PathBuf, String> {
    automation.resolve_user_path(script).map_err(|err| err.to_string())
}

fn run_fuzz(args: &[String]) -> i32 {
    match args {
        [name, file] => match fuzz_targets::run(name, std::path::Path::new(file)) {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("postguy-cli fuzz-target: error: {err}");
                2
            }
        },
        other => {
            eprintln!("postguy-cli fuzz-target: usage: fuzz-target <name> <seed-file> (got {other:?})");
            2
        }
    }
}
