//! Dependency layering (PROMPT.md 4.1), enforced by `cargo xtask layers`.
//!
//! Two rules: a workspace crate may depend only on lower layers, and nothing below L7 may reach
//! a UI toolkit (`egui`, `eframe`, `winit`, ...) through its normal-dependency closure. The rule
//! engine works on a small metadata-independent core so it can be unit-tested;
//! `from_metadata` builds that core from `cargo metadata`.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Workspace crates (package name without the `postguy-` prefix) and their layer. The numbering
/// follows PROMPT.md 4.1 (there is no L5). `testkit` is test-only support but sits at L3 because
/// it hosts real network servers; crates use it as a dev-dependency, which layering does not
/// constrain.
pub const TABLE: &[(&str, u8)] = &[
    ("model", 0),
    ("util", 0),
    ("vars", 1),
    ("urlkit", 1),
    ("auth", 2),
    ("interop", 2),
    ("store", 2),
    ("http", 3),
    ("grpc", 3),
    ("ws", 3),
    ("script", 3),
    ("snippets", 3),
    ("testkit", 3),
    ("runner", 4),
    ("engine", 6),
    ("platform", 7),
    ("ui-egui", 7),
    ("automation", 7),
];

/// External crates that count as a UI toolkit. Prefix match: nothing below L7 may reach any of
/// these through its normal-dependency closure.
pub const UI_CRATES: &[&str] = &["egui", "eframe", "winit", "glow", "glutin", "wgpu", "accesskit", "rfd", "egui_kittest", "egui_extras", "smithay"];

/// Allowed same-layer edges (peer dependencies). Everything same-layer not listed here is a
/// violation, so the layer graph stays a tree deep-down and editors can see what couples.
pub const SIDEWAYS: &[(&str, &str)] = &[("model", "util"), ("ws", "http")];

pub fn short_name(pkg: &str) -> &str {
    pkg.strip_prefix("postguy-").unwrap_or(pkg)
}

pub fn layer_of(short: &str) -> Option<u8> {
    TABLE.iter().find(|(n, _)| *n == short).map(|(_, l)| *l)
}

fn sideways_allowed(from: &str, to: &str) -> bool {
    SIDEWAYS.contains(&(from, to))
}

fn is_ui_crate(name: &str) -> bool {
    UI_CRATES.iter().any(|prefix| name.starts_with(prefix))
}

/// One node of the dependency core: its layer (workspace crates only) and its normal-dependency
/// names.
#[derive(Debug, Default, Clone)]
pub struct Node {
    pub layer: Option<u8>,
    pub deps: Vec<String>,
}

/// Direct layer violations (a workspace crate depending on a higher-layer workspace crate).
pub fn layer_violations(crates: &BTreeMap<String, Node>) -> Vec<String> {
    let mut out = Vec::new();
    for (name, node) in crates {
        let Some(from) = node.layer else { continue };
        for dep in &node.deps {
            let Some(to) = crates.get(dep).and_then(|n| n.layer) else { continue };
            if to > from || (to == from && !sideways_allowed(short_name(name), short_name(dep))) {
                out.push(format!("{} depends on {}", crate_label(name), crate_label(dep)));
            }
        }
    }
    out
}

/// UI-toolkit violations: a crate below L7 whose normal-dependency closure reaches a UI crate.
pub fn ui_violations(crates: &BTreeMap<String, Node>) -> Vec<String> {
    let mut out = Vec::new();
    for (name, node) in crates {
        let Some(layer) = node.layer else { continue };
        if layer >= 7 {
            continue;
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut queue: Vec<&str> = node.deps.iter().map(String::as_str).collect();
        while let Some(dep) = queue.pop() {
            if !seen.insert(dep) {
                continue;
            }
            if is_ui_crate(dep) {
                out.push(format!("{} reaches UI toolkit `{dep}`", crate_label(name)));
            }
            if let Some(next) = crates.get(dep) {
                queue.extend(next.deps.iter().map(String::as_str));
            }
        }
    }
    out
}

fn crate_label(name: &str) -> String {
    format!("{name} (L{})", layer_of(short_name(name)).map_or_else(|| "?".to_string(), |l| l.to_string()))
}

pub fn run(_: &[String]) -> Result<()> {
    let meta = metadata()?;
    let crates = from_metadata(&meta)?;
    let mut problems = layer_violations(&crates);
    problems.extend(ui_violations(&crates));
    if problems.is_empty() {
        println!("layers: {} crates checked, layering holds", TABLE.len());
        return Ok(());
    }
    bail!("layering violated:\n  {}", problems.join("\n  "));
}

/// `cargo metadata --format-version 1` for the workspace.
pub fn metadata() -> Result<Value> {
    let out = std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(crate::gates::root())
        .args(["metadata", "--format-version", "1"])
        .output()
        .context("spawning cargo metadata")?;
    if !out.status.success() {
        bail!("cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    serde_json::from_slice(&out.stdout).context("parsing cargo metadata output")
}

/// Build the rule core from `cargo metadata`: workspace members (and only those with a layer in
/// [`TABLE`]) as nodes, normal dependencies only (`dev` and `build` dependencies do not link into
/// the crate and are not layer-constrained; `testkit` is used as a dev-dependency).
pub fn from_metadata(meta: &Value) -> Result<BTreeMap<String, Node>> {
    let packages = meta["packages"].as_array().context("metadata.packages missing")?;
    let members: &Value = &meta["workspace_members"];
    let members: BTreeSet<&str> = members.as_array().context("metadata.workspace_members missing")?.iter().filter_map(|v| v.as_str()).collect();
    let mut crates: BTreeMap<String, Node> = BTreeMap::new();
    for pkg in packages {
        let name = pkg["name"].as_str().context("package without name")?;
        let id = pkg["id"].as_str().unwrap_or_default();
        let is_member = members.contains(id);
        let Some(layer) = layer_of(short_name(name)).filter(|_| is_member) else { continue };
        let mut deps = Vec::new();
        for dep in pkg["dependencies"].as_array().unwrap_or(&Vec::new()) {
            let kind = dep["kind"].as_str();
            if kind.is_some_and(|k| k == "dev" || k == "build") {
                continue;
            }
            if let Some(dep_name) = dep["name"].as_str() {
                deps.push(dep_name.to_string());
            }
        }
        crates.insert(name.to_string(), Node { layer: Some(layer), deps });
    }
    Ok(crates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(layer: Option<u8>, deps: &[&str]) -> Node {
        Node { layer, deps: deps.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn a_higher_layer_may_depend_on_a_lower_one() {
        let mut crates = BTreeMap::new();
        crates.insert("postguy-model".into(), node(Some(0), &[]));
        crates.insert("postguy-engine".into(), node(Some(6), &["postguy-model"]));
        assert!(layer_violations(&crates).is_empty());
    }

    #[test]
    fn same_layer_needs_a_listed_sideways_edge() {
        let mut crates = BTreeMap::new();
        crates.insert("postguy-model".into(), node(Some(0), &["postguy-util"]));
        crates.insert("postguy-util".into(), node(Some(0), &[]));
        assert!(layer_violations(&crates).is_empty(), "model->util is the listed edge");
        crates.insert("postguy-urlkit".into(), node(Some(1), &["postguy-vars"]));
        crates.insert("postguy-vars".into(), node(Some(1), &[]));
        let violations = layer_violations(&crates);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("postguy-urlkit (L1) depends on postguy-vars (L1)"));
    }

    #[test]
    fn a_lower_layer_may_not_reach_up() {
        let mut crates = BTreeMap::new();
        crates.insert("postguy-model".into(), node(Some(0), &[]));
        crates.insert("postguy-vars".into(), node(Some(1), &[]));
        crates.insert("postguy-engine".into(), node(Some(6), &["postguy-model", "postguy-vars"]));
        crates.insert("postguy-vars".into(), node(Some(1), &["postguy-engine"]));
        let violations = layer_violations(&crates);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("postguy-vars (L1) depends on postguy-engine (L6)"));
    }

    #[test]
    fn a_ui_toolkit_below_l7_is_caught_transitively() {
        let mut crates = BTreeMap::new();
        crates.insert("postguy-vars".into(), node(Some(1), &["serde"]));
        crates.insert("postguy-interop".into(), node(Some(2), &["postguy-vars", "egui"]));
        // serde is not a UI crate; egui is reached transitively.
        let violations = ui_violations(&crates);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("postguy-interop (L2) reaches UI toolkit `egui`"));
    }

    #[test]
    fn l7_and_apps_may_reach_ui_crates() {
        let mut crates = BTreeMap::new();
        crates.insert("postguy-ui-egui".into(), node(Some(7), &["egui"]));
        crates.insert("postguy-vars".into(), node(None, &["egui"])); // no layer: not checked
        assert!(ui_violations(&crates).is_empty());
    }
}
