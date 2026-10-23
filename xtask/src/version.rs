//! `cargo xtask version [set X.Y.Z[-pre]]`: the workspace version, the single source of truth.
//!
//! The version lives in `[workspace.package] version` in the root `Cargo.toml`; every crate
//! inherits it with `version.workspace = true`, and the release packaging reads it from here.

use anyhow::{Context, Result, bail};

/// The `version = "…"` value inside `[workspace.package]`.
pub fn read(manifest: &str) -> Result<String> {
    let (_, line) = find_line(manifest)?;
    let value = line.split_once('=').map(|(_, v)| v.trim().trim_matches('"').to_string()).unwrap_or_default();
    if value.is_empty() {
        bail!("empty [workspace.package] version");
    }
    Ok(value)
}

/// `manifest` with the workspace version replaced; everything else is byte-for-byte unchanged.
pub fn replace(manifest: &str, new: &str) -> Result<String> {
    validate(new)?;
    let (index, _) = find_line(manifest)?;
    let mut out = String::with_capacity(manifest.len() + 8);
    for (i, line) in manifest.split_inclusive('\n').enumerate() {
        if i == index {
            let eol = if line.ends_with("\r\n") {
                "\r\n"
            } else if line.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            out.push_str(&format!("version = \"{new}\"{eol}"));
        } else {
            out.push_str(line);
        }
    }
    Ok(out)
}

/// Semver without build metadata: `MAJOR.MINOR.PATCH` plus an optional `-pre.release` tag.
pub fn validate(version: &str) -> Result<()> {
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    let numbers: Vec<&str> = core.split('.').collect();
    let numeric = |n: &&str| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && (n.len() == 1 || !n.starts_with('0'));
    let pre_ok = pre
        .is_none_or(|pre| !pre.is_empty() && pre.split('.').all(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')));
    if numbers.len() != 3 || !numbers.iter().all(numeric) || !pre_ok {
        bail!("`{version}` is not a version like 1.2.3 or 1.2.3-rc.1");
    }
    Ok(())
}

/// Index and text of the `version` line in `[workspace.package]`.
fn find_line(manifest: &str) -> Result<(usize, &str)> {
    let mut in_section = false;
    for (index, line) in manifest.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[workspace.package]";
            continue;
        }
        if in_section
            && let Some(rest) = trimmed.strip_prefix("version")
            && rest.trim_start().starts_with('=')
        {
            return Ok((index, line));
        }
    }
    bail!("no `version = \"…\"` in [workspace.package] of the root Cargo.toml")
}

pub fn run(args: &[String]) -> Result<()> {
    let root = crate::gates::root();
    let path = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    match args {
        [] => {
            println!("{}", read(&text)?);
            Ok(())
        }
        [set, new] if set == "set" => {
            let old = read(&text)?;
            let updated = replace(&text, new)?;
            // Write beside the manifest and rename: a half-written root manifest breaks every build.
            let temp = root.join("Cargo.toml.xtask-version");
            std::fs::write(&temp, updated).with_context(|| format!("writing {}", temp.display()))?;
            std::fs::rename(&temp, &path).with_context(|| format!("replacing {}", path.display()))?;
            // Refresh the workspace members' entries in Cargo.lock (no dependency upgrades).
            let update = |extra: &[&str]| -> Result<()> {
                let status = std::process::Command::new(env!("CARGO"))
                    .current_dir(&root)
                    .args(["update", "--workspace"])
                    .args(extra)
                    .status()
                    .context("running cargo update")?;
                if status.success() { Ok(()) } else { bail!("cargo update --workspace {} failed", extra.join(" ")) }
            };
            if update(&["--offline"]).is_err() {
                update(&[])?;
            }
            println!("version: {old} -> {new}");
            Ok(())
        }
        _ => bail!("usage: cargo xtask version [set X.Y.Z[-pre]]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "[workspace]\nmembers = [\"a\"]\n\n[workspace.package]\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace.dependencies]\nfoo = { version = \"1\" }\n";

    #[test]
    fn reads_the_workspace_version() {
        assert_eq!(read(MANIFEST).expect("version"), "0.1.0");
        assert!(read("[package]\nversion = \"1.0.0\"\n").is_err());
    }

    #[test]
    fn replaces_only_the_workspace_version() {
        let out = replace(MANIFEST, "1.2.3-rc.1").expect("replace");
        assert_eq!(read(&out).expect("version"), "1.2.3-rc.1");
        assert_eq!(out, MANIFEST.replace("version = \"0.1.0\"", "version = \"1.2.3-rc.1\""));
        assert!(out.contains("foo = { version = \"1\" }"));
    }

    #[test]
    fn keeps_crlf_line_endings() {
        let crlf = MANIFEST.replace('\n', "\r\n");
        let out = replace(&crlf, "2.0.0").expect("replace");
        assert_eq!(out, crlf.replace("\"0.1.0\"", "\"2.0.0\""));
    }

    #[test]
    fn validates_versions() {
        for ok in ["0.1.0", "1.20.300", "1.0.0-rc.1", "1.0.0-alpha", "1.0.0-x-y.2"] {
            assert!(validate(ok).is_ok(), "{ok}");
        }
        for bad in ["1.0", "1.0.0.0", "v1.0.0", "01.0.0", "1.0.0-", "1.0.0-a..b", "1.0.0+meta", "1.a.0", ""] {
            assert!(validate(bad).is_err(), "{bad}");
        }
        assert!(replace(MANIFEST, "nope").is_err());
    }
}
