//! Postguy headless CLI: `run`, `tools`, `call`/`run --script`, `import`/`export`, `snippet`,
//! and the opt-in `mcp` and `ui` subcommands (AGENTS.md 3). Skeleton: the tool table and the
//! `run --script` harness land in M1.8 (PLAN.md); `run` with reporters in M9.3.
//!
//! The `fuzz-target` verb is the child-process entry `cargo xtask fuzz` uses (PLAN M1.9);
//! it is dev tooling, hidden from the human surface, and intentionally NOT part of the
//! automation tool table (fuzz rules need raw survival, not JSON schemas).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod fuzz_targets;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => {
            eprintln!("postguy-cli: the automation tool table is planned for M1.8 (PLAN.md); nothing to run yet.");
        }
        [verb, rest @ ..] if verb == "fuzz-target" => match rest {
            [name, file] => match fuzz_targets::run(name, std::path::Path::new(file)) {
                Ok(()) => {}
                Err(err) => {
                    eprintln!("postguy-cli fuzz-target: error: {err}");
                    std::process::exit(2);
                }
            },
            other => {
                eprintln!("postguy-cli fuzz-target: usage: fuzz-target <name> <seed-file> (got {other:?})");
                std::process::exit(2);
            }
        },
        _ => {
            eprintln!("postguy-cli: unknown subcommand; the automation table arrives in M1.8 (PLAN.md).");
        }
    }
}
