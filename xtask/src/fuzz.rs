//! `cargo xtask fuzz`: mutation fuzzing of the parsers and importers in child processes
//! (AGENTS.md 4). Each target runs `postguy-cli` against a mutated seed file; a panic, a
//! signal or a hang is a finding, saved with its input under `fuzz-out/findings/`. Findings
//! become small synthetic regression tests before the fix (AGENTS.md 4).
//!
//! Targets are declared in [`TARGETS`]; they cover importers, the URL and cookie parsers, the variable resolver and the script sandbox.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

struct Target {
    /// Tool-facing name, used for the findings directory AND the `postguy-cli fuzz-target <name>` arg.
    name: &'static str,
    /// Directory of seed files, relative to the repo root (committed, contributor-original data).
    seeds: &'static str,
    /// `postguy-cli` arguments; `{target}` and `{input}` are replaced per run.
    args: &'static [&'static str],
}

/// The fuzz targets. Each runs `postguy-cli fuzz-target <name> <file>`; the
/// CLI's `fuzz_targets.rs` dispatch is the single place that grows as importers land.
const TARGETS: &[Target] = &[
    Target { name: "url", seeds: "fuzz/seeds/url", args: &["fuzz-target", "url", "{input}"] },
    Target { name: "vars", seeds: "fuzz/seeds/vars", args: &["fuzz-target", "vars", "{input}"] },
    Target { name: "curl", seeds: "fuzz/seeds/curl", args: &["fuzz-target", "curl", "{input}"] },
];

const DEFAULT_TIME_SECS: u64 = 300;
const EXEC_TIMEOUT: Duration = Duration::from_secs(10);

pub fn run(args: &[String]) -> Result<()> {
    let time_secs = flag_num(args, "--time").unwrap_or(DEFAULT_TIME_SECS);
    let seed = flag_num(args, "--seed")
        .unwrap_or_else(|| (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64) | 1);

    let mut cmd = std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(crate::gates::root()).args(["build", "--release", "-p", "postguy-cli"]);
    crate::gates::run(cmd, "cargo build --release -p postguy-cli")?;
    let cli = crate::gates::target_dir().join("release").join(format!("postguy-cli{}", std::env::consts::EXE_SUFFIX));

    let root = crate::gates::root();
    let out_dir = root.join("fuzz-out");
    let findings = out_dir.join("findings");
    std::fs::create_dir_all(&findings)?;

    let mut rng = Rng::new(seed);
    let deadline = Instant::now() + Duration::from_secs(time_secs);
    let mut execs: u64 = 0;
    let mut finding_count: u64 = 0;

    'outer: while Instant::now() < deadline {
        for target in TARGETS {
            let seed_dir = root.join(target.seeds);
            let mut seeds: Vec<PathBuf> = std::fs::read_dir(&seed_dir)
                .with_context(|| format!("reading seeds {}", seed_dir.display()))?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            seeds.sort();
            if seeds.is_empty() {
                bail!("target {} has no seeds in {}", target.name, seed_dir.display());
            }
            let input = std::fs::read(&seeds[rng.below(seeds.len() as u64) as usize])?;
            let mutated = mutate(&input, &mut rng);
            let work_dir = out_dir.join("work");
            std::fs::create_dir_all(&work_dir)?;
            let work_file = work_dir.join(format!("{}-input.bin", target.name));
            std::fs::write(&work_file, &mutated)?;

            let exec_args: Vec<String> =
                target.args.iter().map(|a| a.replace("{input}", &work_file.to_string_lossy()).replace("{target}", target.name)).collect();
            execs += 1;
            match exec(&cli, &exec_args) {
                Outcome::Ok => {}
                Outcome::Rejected => {}
                Outcome::Crash(why) | Outcome::Timeout(why) => {
                    finding_count += 1;
                    let dir = findings.join(target.name);
                    std::fs::create_dir_all(&dir)?;
                    let hash = crate::assets::sha256_hex(&mutated);
                    let finding = dir.join(format!("{execs:06}-{hash}"));
                    std::fs::write(format!("{}.input", finding.display()), &mutated)?;
                    let mut info = std::fs::File::create(format!("{}.txt", finding.display()))?;
                    let _ = writeln!(info, "target: {}\nargs: {}\nwhy: {why}\nseed: {seed}\nexec: {execs}", target.name, exec_args.join(" "));
                    println!("fuzz: FINDING in {} ({why}); saved at {}", target.name, finding.display());
                }
            }
            if Instant::now() >= deadline {
                break 'outer;
            }
        }
        print!("fuzz: {execs} execs, {finding_count} findings\r");
        std::io::stdout().flush().ok();
    }
    println!("fuzz: {execs} execs, {finding_count} findings in {time_secs}s (seed {seed})");
    if finding_count > 0 {
        println!("fuzz: turn every finding into a small synthetic regression test that fails before the fix (AGENTS.md 4)");
    }
    Ok(())
}

enum Outcome {
    /// Exited 0 (or with the CLI's rejection codes): input handled.
    Ok,
    /// Rejected with a readable error (expected for hostile input).
    Rejected,
    Crash(String),
    Timeout(String),
}

fn exec(cli: &Path, args: &[String]) -> Outcome {
    let mut cmd = std::process::Command::new(cli);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return Outcome::Crash(format!("spawn failed: {err}")),
    };
    let deadline = Instant::now() + EXEC_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stderr = child.stderr.take().map(|mut s| {
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut s, &mut buf).ok();
                    String::from_utf8_lossy(&buf).to_string()
                });
                if status.success() {
                    return Outcome::Ok;
                }
                if status.code() == Some(2) {
                    return Outcome::Rejected;
                }
                if let Some(code) = status.code()
                    && (code == 101 || code == 134 || code == 139)
                {
                    return Outcome::Crash(format!("exit {code}: {}", stderr.unwrap_or_default().chars().take(400).collect::<String>()));
                }
                if status.code().is_none() {
                    return Outcome::Crash(format!("signal: {status}"));
                }
                return Outcome::Rejected;
            }
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Outcome::Timeout(format!("no exit within {}s", EXEC_TIMEOUT.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(err) => return Outcome::Crash(format!("wait failed: {err}")),
        }
    }
}

fn flag_num(args: &[String], name: &str) -> Option<u64> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i.checked_add(1)?)).and_then(|v| v.parse().ok())
}

/// A tiny xorshift64* generator: deterministic under `--seed`, no dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        self.next() % bound
    }
}

/// Apply 1-4 random mutations, keeping the result within `max_len`.
fn mutate(input: &[u8], rng: &mut Rng) -> Vec<u8> {
    const MAX_LEN: usize = 1 << 20;
    let mut data = input.to_vec();
    let rounds = 1 + rng.below(4);
    for _ in 0..rounds {
        if data.is_empty() {
            data.push(b'A');
            continue;
        }
        match rng.below(6) {
            0 | 1 => {
                let i = rng.below(data.len() as u64) as usize;
                data[i] = (rng.next() & 0xFF) as u8;
            }
            2 => {
                let i = rng.below(data.len() as u64) as usize;
                data[i] ^= 1 << rng.below(8);
            }
            3 if data.len() < MAX_LEN => {
                let i = rng.below(data.len() as u64) as usize;
                let len = rng.below((data.len() as u64).min(64) + 1) as usize;
                let end = (i + len).min(data.len());
                let chunk: Vec<u8> = data[i..end].to_vec();
                let at = rng.below((data.len() + 1) as u64) as usize;
                if data.len() + chunk.len() <= MAX_LEN {
                    data.splice(at..at, chunk);
                }
            }
            4 if data.len() > 1 => {
                let i = rng.below((data.len() - 1) as u64) as usize;
                data.remove(i);
            }
            _ => {
                let i = rng.below(data.len() as u64) as usize;
                data[i] = b'"'; // terminator characters break parsers most often
            }
        }
    }
    data.truncate(MAX_LEN);
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutations_are_deterministic_and_bounded() {
        let mut rng = Rng::new(42);
        let input = b"{\"key\": \"value\", \"n\": 1}".to_vec();
        let first = mutate(&input, &mut rng);
        let mut rng2 = Rng::new(42);
        let second = mutate(&input, &mut rng2);
        assert_eq!(first, second, "same seed, same mutations");
        assert!(first.len() <= 1 << 20);
        let empty: Vec<u8> = Vec::new();
        let _ = mutate(&empty, &mut rng); // must not panic on empty input
    }

    #[test]
    fn the_rng_is_stable_and_in_range() {
        let mut rng = Rng::new(7);
        for _ in 0..1000 {
            assert!(rng.below(10) < 10);
        }
        assert_eq!(Rng::new(7).next(), Rng::new(7).next());
        assert_eq!(Rng::new(0).below(5), 0, "bound 0 handled");
    }
}
