//! `cargo xtask bench`: run the headless frame-time capture and report the PROMPT.md 8 budgets.
//! The measurement covers our widget tree (layout + paint) without a GPU; run it in release for
//! numbers that compare with the budgets. `--check` fails when a measured budget is exceeded.

use std::collections::BTreeMap;
use std::process::Command;

use anyhow::{Context, Result, bail};

pub fn run(args: &[String]) -> Result<()> {
    let check = args.iter().any(|arg| arg == "--check");
    let debug = args.iter().any(|arg| arg == "--debug");
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(crate::gates::root());
    if debug {
        cmd.args(["run", "-p", "postguy-ui-egui", "--example", "bench"]);
    } else {
        cmd.args(["run", "--release", "-p", "postguy-ui-egui", "--example", "bench"]);
    }
    let output = cmd.output().context("running the bench example")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stdout.lines() {
        println!("{line}");
    }
    if !output.status.success() {
        eprint!("{stderr}");
        bail!("the bench example failed ({})", output.status);
    }
    let mut metrics = BTreeMap::new();
    for line in stdout.lines() {
        if let Some((key, value)) = line.split_once('=')
            && let Ok(value) = value.parse::<f64>()
        {
            metrics.insert(key.trim().to_string(), value);
        }
    }
    if metrics.is_empty() {
        bail!("the bench example printed no key=value metrics");
    }
    if !check {
        println!("bench: measurements reported (pass --check to enforce the budgets)");
        return Ok(());
    }
    let budgets = [
        ("tree.first_ms", 300.0, "10 000-request tree usable"),
        ("tree.filter_ms", 16.0, "filter keystroke"),
        ("tree.avg_ms", 8.0, "typical frame"),
        ("response.first_raw_ms", 200.0, "10 MB Raw first frame"),
        ("response.avg_raw_ms", 33.0, "10 MB Raw frame, never over 33 ms"),
    ];
    let mut over = Vec::new();
    for (key, budget, what) in budgets {
        match metrics.get(key) {
            Some(value) if *value > budget => over.push(format!("{key} {value:.2} ms > {budget:.0} ms ({what})")),
            Some(value) => println!("bench: {key} {value:.2} ms <= {budget:.0} ms ({what})"),
            None => over.push(format!("{key} missing from the bench output")),
        }
    }
    if over.is_empty() {
        println!("bench: all checked budgets met");
        return Ok(());
    }
    bail!("budgets exceeded:\n  {}", over.join("\n  "))
}
