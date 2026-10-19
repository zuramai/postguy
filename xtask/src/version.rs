//! `cargo xtask version`: print the workspace version (root Cargo.toml `[workspace.package]`).

use anyhow::{Context, Result};

pub fn run(_: &[String]) -> Result<()> {
    let text = std::fs::read_to_string(crate::gates::root().join("Cargo.toml")).context("reading Cargo.toml")?;
    let value: toml::Value = toml::from_str(&text).context("parsing Cargo.toml")?;
    let version = value
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .context("no [workspace.package] version")?;
    println!("{version}");
    Ok(())
}
