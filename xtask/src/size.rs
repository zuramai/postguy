//! `cargo xtask size`: measure the release binaries against the size budgets.
//!
//! Budgets (macOS arm64, stripped, enforced in CI): desktop app <= 18 MB with `scripting` on (stretch 14; `scripting` off <= 12 MB). The
//! stretch number is reported, not gated.

use anyhow::{Context, Result, bail};

fn binary(name: &str) -> anyhow::Result<std::path::PathBuf> {
    let path = crate::gates::target_dir().join("release").join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    if !path.exists() {
        bail!("{} is missing; build it first (cargo build --release -p {name})", path.display());
    }
    Ok(path)
}

pub fn run(args: &[String]) -> Result<()> {
    let skip_build = args.iter().any(|a| a == "--skip-build");
    if !skip_build {
        let mut cmd = std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cmd.current_dir(crate::gates::root()).args(["build", "--release", "-p", "postguy", "-p", "postguy-cli"]);
        crate::gates::run(cmd, "cargo build --release")?;
    }

    let app_budget: f64 = 18.0 * 1024.0 * 1024.0;
    let app_stretch: f64 = 14.0 * 1024.0 * 1024.0;
    let mut over = Vec::new();
    for name in ["postguy", "postguy-cli"] {
        let path = binary(name)?;
        let size = std::fs::metadata(&path).with_context(|| format!("statting {}", path.display()))?.len();
        let mib = size as f64 / (1024.0 * 1024.0);
        match name {
            "postguy" => println!("size: {name} {mib:.2} MiB (budget 18.00 MiB, stretch 14.00 MiB)"),
            _ => println!("size: {name} {mib:.2} MiB (no budget yet)"),
        }
        if name == "postguy" {
            if size as f64 > app_budget {
                over.push(format!("postguy is {mib:.2} MiB, over the 18 MiB budget"));
            } else if size as f64 > app_stretch {
                println!("size: postguy is over the 14 MiB stretch target (budget still met)");
            }
        }
    }
    if let Ok(out) = std::process::Command::new("cargo").args(["bloat", "--version"]).output()
        && out.status.success()
    {
        println!("size: cargo-bloat installed; run `cargo bloat --release -p postguy --crates` for the breakdown");
    } else {
        println!("size: cargo-bloat not installed; breakdown skipped");
    }
    if over.is_empty() {
        return Ok(());
    }
    bail!("size budgets exceeded:\n  {}", over.join("\n  "));
}
