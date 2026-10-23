//! Project automation for Postguy, run via `cargo xtask <command>`.

use std::process::ExitCode;

mod assets;
mod bench;
mod deny;
mod fuzz;
mod gates;
mod icons;
mod layers;
mod logo;
mod parity;
mod screenshots;
mod size;
mod version;

type Command = fn(&[String]) -> anyhow::Result<()>;

/// Every subcommand: name, one-line summary, entry point.
const COMMANDS: &[(&str, &str, Command)] = &[
    ("layers", "Enforce the crate dependency layering", gates::layers),
    ("assets", "Enforce the asset policy (AGENTS.md 1) against ATTRIBUTION.toml; --write regenerates ATTRIBUTION.md", assets::run),
    ("parity", "Validate parity/postman-features.toml against the registry, tools and tests; report progress (--json, --partial)", parity::run),
    ("deny", "Dependency licences and sources (deny.toml; delegates to cargo-deny when installed)", deny::run),
    ("size", "Measure the release binaries against the size budgets", size::run),
    ("bench", "Headless frame-time capture; reports (--check enforces) the frame-time budgets", bench::run),
    ("icons", "Regenerate crates/ui-egui/src/icons_gen.rs from assets/icons/*.svg (--check verifies)", icons::run),
    ("logo", "Rasterize assets/logo/postguy.svg into the packaging PNG sizes (--check verifies)", logo::run),
    ("fuzz", "Mutation-fuzz the parsers and importers in child processes; findings in fuzz-out/ (--time 300)", fuzz::run),
    ("screenshots", "Regenerate the docs screenshots from our own UI with demo data", screenshots::run),
    ("version", "Print the workspace version, or `version set X.Y.Z` to change it", version::run),
    ("ci", "fmt --check, clippy -D warnings, test, layers, assets, parity, deny (stops at first failure)", gates::ci),
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(name) = args.first() else {
        print_help();
        return ExitCode::FAILURE;
    };
    if matches!(name.as_str(), "help" | "-h" | "--help") {
        print_help();
        return ExitCode::SUCCESS;
    }
    let Some((_, _, command)) = COMMANDS.iter().find(|(n, _, _)| n == name) else {
        eprintln!("xtask: unknown command `{name}`\n");
        print_help();
        return ExitCode::FAILURE;
    };
    match command(&args[1..]) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask {name}: error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    eprintln!("Usage: cargo xtask <command> [args]\n\nCommands:");
    for (name, about, _) in COMMANDS {
        eprintln!("  {name:<12} {about}");
    }
    eprintln!("  {:<12} Show this list", "help");
}
