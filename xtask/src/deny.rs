//! `cargo xtask deny`: dependency licences and sources.
//!
//! When `cargo-deny` is installed it delegates to it (deny.toml is the policy). Otherwise —
//! when the tool is not installed — this module
//! checks the same core rules itself from `cargo metadata`: every dependency's licence is on the
//! allowlist in deny.toml, no crate is a forbidden internal dependency, and no dependency comes
//! from a git source. Advisories still need cargo-deny (or CI's cargo-deny-action).

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Deserialize, Default)]
#[serde(default)]
struct DenyToml {
    licenses: Licenses,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Licenses {
    allow: Vec<String>,
    exceptions: Vec<Exception>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exception {
    #[serde(rename = "crate")]
    krate: String,
    allow: Vec<String>,
}

/// Internal dependencies of another team: never allowed, under any licence.
const BANNED_CRATES: &[&str] = &["craft-ui", "craft-libs", "craft-print", "craft-quit", "craft-docpath", "craft-fonts"];

fn root() -> std::path::PathBuf {
    crate::gates::root()
}

pub fn run(_: &[String]) -> Result<()> {
    let mut probe = std::process::Command::new("cargo");
    probe.args(["deny", "--version"]);
    if let Ok(out) = probe.output()
        && out.status.success()
    {
        return crate::gates::run(
            {
                let mut cmd = std::process::Command::new("cargo");
                cmd.current_dir(root());
                cmd.args(["deny", "check"]);
                cmd
            },
            "cargo deny check",
        );
    }

    own_check()
}

fn own_check() -> Result<()> {
    let text = std::fs::read_to_string(root().join("deny.toml")).context("reading deny.toml")?;
    let policy: DenyToml = toml::from_str(&text).context("parsing deny.toml")?;
    if policy.licenses.allow.is_empty() {
        bail!("deny.toml has no [licenses] allow list");
    }
    let allow: Vec<&str> = policy.licenses.allow.iter().map(String::as_str).collect();

    let meta = crate::layers::metadata()?;
    let packages = meta["packages"].as_array().context("metadata.packages missing")?;
    let members: std::collections::BTreeSet<&str> =
        meta["workspace_members"].as_array().context("metadata.workspace_members missing")?.iter().filter_map(|v| v.as_str()).collect();

    let mut problems: Vec<String> = Vec::new();
    for pkg in packages {
        let name = pkg["name"].as_str().context("package without name")?;
        let id = pkg["id"].as_str().unwrap_or_default();
        if members.contains(id) {
            continue; // our own crates are MIT OR Apache-2.0 by workspace declaration
        }
        if BANNED_CRATES.contains(&name) {
            problems.push(format!("{name}: dependency on a forbidden internal crate (see deny.toml)"));
        }
        let licence = pkg["license"].as_str();
        let Some(licence) = licence else {
            problems.push(format!("{name}: no licence field (cannot verify; AGENTS.md allows permissive licences only)"));
            continue;
        };
        let exception = policy.licenses.exceptions.iter().find(|e| e.krate == name);
        let mut allowed: Vec<&str> = allow.clone();
        if let Some(exception) = exception {
            allowed.extend(exception.allow.iter().map(String::as_str));
        }
        if !crate::assets::licence_allowed(licence, &allowed, &["LLVM-exception"]) {
            problems.push(format!("{name}: licence `{licence}` is not on the deny.toml allowlist"));
        }
        if let Some(source) = pkg["source"].as_str()
            && source.starts_with("git+")
        {
            problems.push(format!("{name}: git dependency `{source}` (deny.toml [sources] denies unknown git sources)"));
        }
    }

    if problems.is_empty() {
        println!(
            "deny: {} packages checked against the deny.toml allowlist (cargo-deny not installed; advisories unchecked — install cargo-deny or rely on CI)",
            packages.len()
        );
        return Ok(());
    }
    bail!("dependency policy violated:\n  {}", problems.join("\n  "));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_parses_deny_toml() {
        let text = std::fs::read_to_string(root().join("deny.toml")).expect("deny.toml exists");
        let policy: DenyToml = toml::from_str(&text).expect("deny.toml parses");
        assert!(policy.licenses.allow.contains(&"MIT".to_string()));
        assert!(policy.licenses.exceptions.iter().any(|e| e.krate == "epaint_default_fonts"));
    }
}
