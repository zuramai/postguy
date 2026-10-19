//! `cargo xtask parity`: validate `parity/postman-features.toml` and report progress.
//!
//! The file is the honest checklist (AGENTS.md 3): one `[[feature]]` per user-facing feature.
//! `shipped` requires cited passing tests whose paths and test names exist, and commands/tools
//! that exist in the `engine` and `automation` registries; `planned` and `na` entries must not
//! cite evidence; `na` needs a reason in `notes`.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Parity {
    #[serde(default)]
    schema: Schema,
    #[serde(default)]
    feature: Vec<Feature>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Schema {
    version: u8,
    #[serde(default)]
    statuses: Vec<String>,
    #[serde(default)]
    tiers: Vec<String>,
    #[serde(default)]
    areas: std::collections::BTreeMap<String, String>,
}

impl Default for Schema {
    fn default() -> Self {
        Self {
            version: 1,
            statuses: ["planned", "partial", "shipped", "na"].iter().map(|s| s.to_string()).collect(),
            tiers: ["P0", "P1", "P2", "P3", "N/A"].iter().map(|s| s.to_string()).collect(),
            areas: std::collections::BTreeMap::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Feature {
    id: String,
    title: String,
    area: String,
    tier: String,
    milestone: String,
    status: String,
    #[serde(default)]
    commands: Vec<String>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    notes: String,
}

fn root() -> std::path::PathBuf {
    crate::gates::root()
}

pub fn run(args: &[String]) -> Result<()> {
    let json = args.iter().any(|a| a == "--json");
    let partial = args.iter().any(|a| a == "--partial");
    let root = root();
    let path = root.join("parity").join("postman-features.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let parity: Parity = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

    let statuses = if parity.schema.statuses.is_empty() { Schema::default().statuses } else { parity.schema.statuses };
    let tiers = if parity.schema.tiers.is_empty() { Schema::default().tiers } else { parity.schema.tiers };
    if parity.schema.version != 1 {
        bail!("parity schema version {} is not 1; update the validator", parity.schema.version);
    }

    let mut problems: Vec<String> = Vec::new();
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for f in &parity.feature {
        if !seen.insert(&f.id) {
            problems.push(format!("duplicate feature id `{}`", f.id));
        }
        if !parity.schema.areas.contains_key(&f.area) {
            problems.push(format!("{}: unknown area `{}` (not in the [schema] areas table)", f.id, f.area));
        }
        if !statuses.contains(&f.status) {
            problems.push(format!("{}: unknown status `{}` (allowed: {})", f.id, f.status, statuses.join(", ")));
        }
        if !tiers.contains(&f.tier) {
            problems.push(format!("{}: unknown tier `{}`", f.id, f.tier));
        }
        let milestone_ok = match f.milestone.strip_prefix('M') {
            None => f.milestone.is_empty(),
            Some(rest) => !rest.is_empty() && rest.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())),
        };
        if !milestone_ok {
            problems.push(format!("{}: milestone `{}` does not look like M2 or M2.3", f.id, f.milestone));
        }
        let exists = |evidence: &str| -> bool {
            let Some((path, test)) = evidence.split_once("::") else { return Path::new(&root).join(evidence).exists() };
            let file = root.join(path);
            match std::fs::read_to_string(&file) {
                Ok(text) => text.contains(&format!("fn {test}")),
                Err(_) => false,
            }
        };
        for e in &f.evidence {
            if !exists(e) {
                problems.push(format!("{}: evidence {} does not exist (path or test name)", f.id, e));
            }
        }
        match f.status.as_str() {
            "shipped" => {
                if f.evidence.is_empty() {
                    problems.push(format!("{}: shipped without cited evidence (a passing test)", f.id));
                }
            }
            "planned" => {
                if !f.evidence.is_empty() {
                    problems.push(format!("{}: planned entries must not cite evidence", f.id));
                }
                if !f.commands.is_empty() || !f.tools.is_empty() {
                    problems.push(format!("{}: planned entries must not cite commands or tools that do not exist yet", f.id));
                }
            }
            "na" if f.notes.trim().is_empty() => {
                problems.push(format!("{}: na entries must name the reason (or the local alternative) in notes", f.id));
            }
            _ => {}
        }
        for id in &f.commands {
            if !text_exists(&root.join("crates").join("engine"), id) {
                problems.push(format!("{}: command `{id}` not found in crates/engine/src", f.id));
            }
        }
        for tool in &f.tools {
            if !text_exists(&root.join("crates").join("automation"), tool) {
                problems.push(format!("{}: tool `{tool}` not found in crates/automation/src", f.id));
            }
        }
    }

    let total = parity.feature.len();
    let count = |status: &str| parity.feature.iter().filter(|f| f.status == status).count();
    if json {
        let mut out = String::new();
        let _ = writeln!(out, "{{");
        for f in &parity.feature {
            let _ = writeln!(
                out,
                "  {{ \"id\": \"{}\", \"status\": \"{}\", \"tier\": \"{}\", \"milestone\": \"{}\" }},",
                f.id, f.status, f.tier, f.milestone
            );
        }
        let _ = writeln!(out, "  \"total\": {total}");
        let _ = writeln!(out, "}}");
        print!("{out}");
    } else if partial {
        for f in &parity.feature {
            if f.status != "shipped" {
                println!("{:<24} {:>8} {:>3} {:<6} {}", f.id, f.status, f.tier, f.milestone, f.title);
            }
        }
    } else {
        println!(
            "parity: {total} features — {} shipped, {} partial, {} planned, {} na",
            count("shipped"),
            count("partial"),
            count("planned"),
            count("na")
        );
    }

    if problems.is_empty() {
        return Ok(());
    }
    bail!("parity file invalid:\n  {}", problems.join("\n  "));
}

/// Whether `needle` appears in any .rs file under `dir` (recursive).
fn text_exists(dir: &Path, needle: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if text_exists(&path, needle) {
                return true;
            }
        } else if path.extension().is_some_and(|e| e == "rs") && std::fs::read_to_string(&path).is_ok_and(|text| text.contains(needle)) {
            return true;
        }
    }
    false
}
