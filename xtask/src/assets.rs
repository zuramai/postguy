//! `cargo xtask assets`: enforce the asset policy (AGENTS.md 1).
//!
//! `ATTRIBUTION.toml` lists every asset: committed files (`[[asset]]`) and files that Cargo
//! dependencies compile into Postguy (`[[bundled]]`). The gate fails when:
//! - an asset-like file in the repository has no entry, or its SHA-256 differs;
//! - an entry's file does not exist, its licence is not on the allowlist, or the licence file is
//!   missing;
//! - an icon, image or font names Postman as author or source, or a `postman_data` entry is not
//!   on the closed list approved in AGENTS.md 1.1;
//! - a `[[bundled]]` crate version is not the one in Cargo.lock, or the file's hash differs;
//! - `ATTRIBUTION.md` is out of date (`--write` regenerates it).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Licences allowed by AGENTS.md 1.2 (SPDX identifiers).
pub const ALLOWED_LICENCES: &[&str] = &[
    "ISC",
    "MIT",
    "MIT-0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "Apache-2.0",
    "OFL-1.1",
    "Ubuntu-font-1.0",
    "Bitstream-Vera",
    "CC0-1.0",
    "CC-BY-4.0",
    "CC-BY-SA-4.0",
    "LicenseRef-Public-Domain",
];

/// The closed list of Postman-authored, non-visual technical data (AGENTS.md 1.1), as repo paths.
/// Adding anything needs the owner's explicit approval; anything else with `postman_data = true`
/// fails here. The two entries are the externally published Collection Format schemas
/// (Apache-2.0), which AGENTS.md 1.1 authorizes for import/export validation.
pub const POSTMAN_DATA_ALLOWED: &[&str] = &["crates/interop/testdata/collection-v21.json", "crates/interop/testdata/collection-v20.json"];

/// File extensions that count as assets wherever they appear in the repository.
const ASSET_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "svg", "ico", "icns", "webp", "bmp", "tif", "tiff", "avif", "heic", "ttf", "otf", "ttc", "woff", "woff2", "pfb",
    "pfa", "afm", "icc", "icm", "pdf", "eps", "ps", "ai", "psd", "mp3", "wav", "ogg", "flac", "mp4", "mov", "webm", "cur", "ani",
];

/// Kinds where Postman authorship would breach AGENTS.md 1.1 (no Postman visual design at all).
const VISUAL_KINDS: &[&str] = &["icon", "image", "font", "logo", "screenshot", "illustration"];

/// Directories never entered by the scan (build outputs and local-only notes; the local
/// `plan/` folder stays uncommitted).
const SKIPPED_DIRS: &[&str] = &[".git", "target", "dist", "plan", "fuzz-out", "node_modules"];

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub asset: Vec<Asset>,
    #[serde(default)]
    pub bundled: Vec<Bundled>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub path: String,
    pub title: String,
    pub author: String,
    pub source: String,
    pub licence: String,
    pub licence_file: String,
    pub kind: String,
    pub usage: String,
    pub sha256: String,
    #[serde(default)]
    pub postman_data: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundled {
    #[serde(rename = "crate")]
    pub krate: String,
    pub version: String,
    pub path: String,
    pub title: String,
    pub author: String,
    pub source: String,
    pub licence: String,
    pub kind: String,
    pub usage: String,
    pub sha256: String,
    #[serde(default)]
    pub postman_data: bool,
}

pub fn root() -> PathBuf {
    crate::gates::root()
}

pub fn load(root: &Path) -> Result<Manifest> {
    let path = root.join("ATTRIBUTION.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for byte in digest {
        let _ = write!(s, "{byte:02x}");
    }
    s
}

/// Whether an SPDX expression is entirely on the allowlist. Supports `OR` (any alternative may
/// pass), `AND` (all tokens must pass) and `WITH` exceptions. `/` (crates.io's informal
/// dual-licence notation, e.g. `MIT/Apache-2.0`) is treated like `OR`: it grants a choice
/// between the licences.
pub fn licence_allowed(expression: &str, allow: &[&str], exceptions: &[&str]) -> bool {
    let expression = expression.replace(['(', ')'], " ");
    expression.split(" OR ").flat_map(|alternative| alternative.split('/')).any(|alternative| {
        alternative.split(" AND ").all(|token| {
            let token = token.trim();
            let (base, exception) = match token.split_once(" WITH ") {
                Some((base, exception)) => (base.trim(), exception.trim()),
                None => (token, ""),
            };
            allow.contains(&base) && (exception.is_empty() || exceptions.contains(&exception))
        })
    })
}

pub fn run(args: &[String]) -> Result<()> {
    let write = args.iter().any(|a| a == "--write");
    let root = root();
    let manifest = load(&root)?;

    let mut problems: Vec<String> = Vec::new();

    // Every asset-like file in the repository needs an entry with a matching hash.
    let mut files: Vec<PathBuf> = Vec::new();
    scan(&root, &mut files)?;
    let entries: std::collections::BTreeMap<&str, &Asset> = manifest.asset.iter().map(|a| (a.path.as_str(), a)).collect();
    for file in &files {
        let rel = file.strip_prefix(&root).expect("scan roots at repo root").to_string_lossy().replace('\\', "/");
        let Some(entry) = entries.get(rel.as_str()) else {
            problems.push(format!("asset file {rel} has no ATTRIBUTION.toml entry"));
            continue;
        };
        check_file_hash(&root, &rel, &entry.sha256, &mut problems);
    }
    for entry in &manifest.asset {
        // Existence is a filesystem check (word lists live in source files, which the
        // asset-extension scan does not drop into `files`).
        if !root.join(&entry.path).exists() {
            problems.push(format!("ATTRIBUTION.toml entry `{}` names a missing file {}", entry.title, entry.path));
        }
        check_entry(
            &root,
            entry.path.as_str(),
            &entry.licence,
            &entry.licence_file,
            &entry.kind,
            &entry.author,
            &entry.source,
            entry.postman_data,
            &mut problems,
        );
    }

    // Bundled (Cargo-dependency) assets: version from Cargo.lock, hash from the registry source.
    let lock = read_lock(&root)?;
    for bundled in &manifest.bundled {
        let found = lock.iter().any(|(n, v)| n == &bundled.krate && v == &bundled.version);
        if !found {
            let versions: Vec<&str> = lock.iter().filter(|(n, _)| n == &bundled.krate).map(|(_, v)| v.as_str()).collect();
            problems.push(format!(
                "bundled crate {} is not at version {} in Cargo.lock (found: {})",
                bundled.krate,
                bundled.version,
                if versions.is_empty() { "not in the lockfile".to_string() } else { versions.join(", ") }
            ));
            continue;
        }
        match registry_file(&bundled.krate, &bundled.version, &bundled.path) {
            Some(file) => {
                let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
                let hash = sha256_hex(&bytes);
                if hash != bundled.sha256 {
                    problems.push(format!(
                        "bundled asset {} {} {} has SHA-256 {}, ATTRIBUTION.toml says {}",
                        bundled.krate, bundled.version, bundled.path, hash, bundled.sha256
                    ));
                }
            }
            None => problems.push(format!(
                "bundled asset {} {} not found in the cargo registry source (run `cargo fetch`); missing: {}",
                bundled.krate, bundled.version, bundled.path
            )),
        }
        check_entry(
            &root,
            &format!("{} {}", bundled.krate, bundled.path),
            &bundled.licence,
            "",
            &bundled.kind,
            &bundled.author,
            &bundled.source,
            bundled.postman_data,
            &mut problems,
        );
    }

    // ATTRIBUTION.md must be the generated rendering of ATTRIBUTION.toml.
    let markdown = render_markdown(&manifest);
    let md_path = root.join("ATTRIBUTION.md");
    if write {
        std::fs::write(&md_path, &markdown)?;
        println!("assets: wrote ATTRIBUTION.md");
    } else {
        let current = std::fs::read_to_string(&md_path).unwrap_or_default();
        if current != markdown {
            problems.push("ATTRIBUTION.md is out of date with ATTRIBUTION.toml (run `cargo xtask assets --write`)".to_string());
        }
    }

    if problems.is_empty() {
        println!("assets: {} committed, {} bundled, all attributed", manifest.asset.len(), manifest.bundled.len());
        return Ok(());
    }
    bail!("asset policy violated:\n  {}", problems.join("\n  "));
}

fn check_file_hash(root: &Path, rel: &str, expected: &str, problems: &mut Vec<String>) {
    let bytes = match std::fs::read(root.join(rel)) {
        Ok(bytes) => bytes,
        Err(err) => {
            problems.push(format!("asset file {rel} cannot be read: {err}"));
            return;
        }
    };
    let hash = sha256_hex(&bytes);
    if hash != expected {
        problems.push(format!("asset file {rel} has SHA-256 {hash}, ATTRIBUTION.toml says {expected}"));
    }
}

#[allow(clippy::too_many_arguments)]
fn check_entry(
    root: &Path,
    label: &str,
    licence: &str,
    licence_file: &str,
    kind: &str,
    author: &str,
    source: &str,
    postman_data: bool,
    problems: &mut Vec<String>,
) {
    if !licence_allowed(licence, ALLOWED_LICENCES, &["LLVM-exception"]) {
        problems.push(format!("{label}: licence `{licence}` is not on the AGENTS.md 1.2 allowlist"));
    }
    if !licence_file.is_empty() && !root.join(licence_file).exists() {
        problems.push(format!("{label}: licence file {licence_file} is missing"));
    }
    let provenance = format!("{author} {source}").to_lowercase();
    if VISUAL_KINDS.contains(&kind) && provenance.contains("postman") {
        problems.push(format!("{label}: {kind} names Postman as author or source (AGENTS.md 1.1 forbids Postman visual material)"));
    }
    if postman_data && !POSTMAN_DATA_ALLOWED.iter().any(|allowed| label.contains(allowed)) {
        problems.push(format!("{label}: postman_data entries are on a closed list approved by the owner; `{label}` is not on it (AGENTS.md 1.1)"));
    }
}

fn scan(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .collect::<std::result::Result<Vec<std::fs::DirEntry>, _>>()
        .with_context(|| format!("listing {}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries.into_iter().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        if path.is_dir() {
            if !SKIPPED_DIRS.contains(&name.as_str()) {
                scan(&path, out)?;
            }
        } else if path.is_file() && path.extension().and_then(|e| e.to_str()).is_some_and(|ext| ASSET_EXTENSIONS.contains(&ext)) {
            out.push(path);
        }
    }
    Ok(())
}

/// (name, version) for every package in Cargo.lock.
fn read_lock(root: &Path) -> Result<Vec<(String, String)>> {
    let text = std::fs::read_to_string(root.join("Cargo.lock")).context("reading Cargo.lock")?;
    #[derive(Deserialize)]
    struct LockPackage {
        name: String,
        version: String,
    }
    #[derive(Deserialize)]
    struct Lock {
        #[serde(default, rename = "package")]
        packages: Vec<LockPackage>,
    }
    let lock: Lock = toml::from_str(&text).context("parsing Cargo.lock")?;
    Ok(lock.packages.into_iter().map(|p| (p.name, p.version)).collect())
}

/// Locate a file of a crate version inside the local cargo registry source tree.
fn registry_file(krate: &str, version: &str, rel: &str) -> Option<PathBuf> {
    let cargo_home =
        std::env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))?;
    let src = cargo_home.join("registry").join("src");
    let dir = std::fs::read_dir(&src).ok()?;
    for entry in dir.flatten() {
        let candidate = entry.path().join(format!("{krate}-{version}")).join(rel);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

fn render_markdown(manifest: &Manifest) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Third-party assets");
    let _ = writeln!(out);
    let _ = writeln!(out, "Generated by `cargo xtask assets --write` from ATTRIBUTION.toml; do not edit by hand.");
    let _ = writeln!(out);
    let _ = writeln!(out, "## Committed assets");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Path | Title | Author | Source | Licence | Kind | Used for |");
    let _ = writeln!(out, "|---|---|---|---|---|---|---|");
    for a in &manifest.asset {
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} | {} |",
            cell(&a.path),
            cell(&a.title),
            cell(&a.author),
            cell(&a.source),
            cell(&a.licence),
            cell(&a.kind),
            cell(&a.usage)
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "## Bundled by Cargo dependencies");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Crate | Version | File | Title | Author | Licence | Kind | Used for |");
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|");
    for b in &manifest.bundled {
        let _ = writeln!(
            out,
            "| `{}` | {} | `{}` | {} | {} | {} | {} | {} |",
            cell(&b.krate),
            cell(&b.version),
            cell(&b.path),
            cell(&b.title),
            cell(&b.author),
            cell(&b.licence),
            cell(&b.kind),
            cell(&b.usage)
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compound_licences_pass_when_every_token_is_allowed() {
        assert!(licence_allowed("MIT OR Apache-2.0", ALLOWED_LICENCES, &["LLVM-exception"]));
        assert!(licence_allowed("(MIT OR Apache-2.0) WITH LLVM-exception", ALLOWED_LICENCES, &["LLVM-exception"]));
        assert!(licence_allowed("MIT AND BSD-3-Clause", ALLOWED_LICENCES, &["LLVM-exception"]));
        assert!(licence_allowed("MIT/Apache-2.0", ALLOWED_LICENCES, &["LLVM-exception"]));
        assert!(licence_allowed("Apache-2.0/MIT", ALLOWED_LICENCES, &["LLVM-exception"]));
        assert!(licence_allowed("MIT / Apache-2.0", ALLOWED_LICENCES, &["LLVM-exception"]));
    }

    #[test]
    fn copyleft_and_unknown_licences_fail() {
        assert!(!licence_allowed("GPL-3.0-only", ALLOWED_LICENCES, &["LLVM-exception"]));
        // `OR` is a choice the licensor grants: chooses MIT, chooses lawfully.
        assert!(licence_allowed("LGPL-3.0 OR MIT", ALLOWED_LICENCES, &["LLVM-exception"]));
        assert!(!licence_allowed("AGPL-3.0", ALLOWED_LICENCES, &["LLVM-exception"]));
        assert!(!licence_allowed("Freeware", ALLOWED_LICENCES, &["LLVM-exception"]));
        assert!(!licence_allowed("MIT WITH GPL-3.0", ALLOWED_LICENCES, &["LLVM-exception"]));
    }
}
