//! `cargo xtask screenshots`: render the real shell through the headless `shot` example with
//! demo data (AGENTS.md 1.2: screenshots are contributor-original; everything visible in them
//! must be allowed). Output lands in `target/screenshots/`, which is ignored: they are review
//! artifacts, not published assets. Publishing them (M10.4) needs an ATTRIBUTION.toml entry per
//! file. `POSTGUY_SYSTEM_FONTS=0` keeps the runtime font fallback out of every image (AGENTS.md 1.4).

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// (mode passed to the example, file name under the output directory).
const SETS: &[(&str, &str)] = &[("dark", "shell-dark.png"), ("light", "shell-light.png"), ("env", "environment-dark.png")];

pub fn run(args: &[String]) -> Result<()> {
    let out = match args.iter().position(|arg| arg == "--out").and_then(|index| args.get(index + 1)) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from("target/screenshots"),
    };
    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    for (mode, file) in SETS {
        let path = out.join(file);
        let status = Command::new(env!("CARGO"))
            .args(["run", "-p", "postguy-ui-egui", "--example", "shot", "--"])
            .arg(&path)
            .arg(mode)
            .env("POSTGUY_SYSTEM_FONTS", "0")
            .status()
            .context("running the shot example")?;
        if !status.success() {
            bail!("the shot example failed for {mode} ({status})");
        }
        if !path.is_file() {
            bail!("the shot example reported success but {} is missing", path.display());
        }
        println!("screenshots: {}", path.display());
    }
    Ok(())
}
