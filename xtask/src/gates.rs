//! Quality gates: the `ci` bundle and shared process helpers.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask has a parent dir").to_path_buf()
}

/// Cargo's target directory: `CARGO_TARGET_DIR` when it is set (parallel agents use their own),
/// else `target/`.
pub fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => root().join("target"),
    }
}

fn cargo() -> Command {
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root());
    cmd
}

pub fn run(mut cmd: Command, what: &str) -> Result<()> {
    let status = cmd.status().with_context(|| format!("spawning {what}"))?;
    if !status.success() {
        bail!("{what} failed ({status})");
    }
    Ok(())
}

pub fn layers(args: &[String]) -> Result<()> {
    crate::layers::run(args)
}

/// `cargo xtask ci`: the CI bundle in CI's order. Local development scopes builds and tests to
/// the crates touched under a timeout (AGENTS.md 5.3); this is the full sweep CI runs.
pub fn ci(_: &[String]) -> Result<()> {
    run(
        {
            let mut cmd = cargo();
            cmd.args(["fmt", "--all", "--", "--check"]);
            cmd
        },
        "cargo fmt --check",
    )?;
    run(
        {
            let mut cmd = cargo();
            cmd.args(["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]);
            cmd
        },
        "cargo clippy",
    )?;
    run(
        {
            let mut cmd = cargo();
            cmd.args(["test", "--workspace"]);
            cmd
        },
        "cargo test",
    )?;
    crate::layers::run(&[])?;
    crate::assets::run(&[])?;
    crate::parity::run(&[])?;
    crate::deny::run(&[])?;
    println!("ci: all gates green");
    Ok(())
}
