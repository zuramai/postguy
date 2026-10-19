//! Fuzz targets for `cargo xtask fuzz` (PLAM M1.9): each is a pure parser entry the fuzzer's
//! child process calls with a mutated seed. No network, no filesystem writes; a panic here is
//! a BUG per AGENTS.md 4 and the violated `#![deny]` contract defended below.
//!
//! Targets registered in `xtask/src/fuzz.rs`'s `TARGETS` map to these names; each takes the
//! seed PATH (already size-capped by the driver) and swallows all outcomes — the point is the
//! process's survival, not the parse result.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Registered target names (keeps the xtask table and this dispatch one source).
pub const NAMES: &[&str] = &["url", "vars", "curl"];

/// Run one target over one file; `Ok(())` = the crate survived. Any panic escapes and the
/// driver records a finding.
pub fn run(name: &str, file: &std::path::Path) -> Result<(), String> {
    let bytes = read_capped(file)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    match name {
        "url" => url_target(&text),
        "vars" => vars_target(&text),
        "curl" => curl_target(&text),
        other => Err(format!("unknown fuzz target `{other}` (registered: {NAMES:?})")),
    }
}

fn read_capped(file: &std::path::Path) -> Result<Vec<u8>, String> {
    postguy_util::read_file_capped(file, 8 << 20).map_err(|e| format!("{e}"))
}

fn url_target(text: &str) -> Result<(), String> {
    let url = postguy_urlkit::Url::parse(text);
    let _ = url.build();
    let _ = postguy_urlkit::path_vars_of(text);
    let _ = postguy_urlkit::params_of_query(text);
    let _ = postguy_urlkit::encode_path(text);
    let _ = postguy_urlkit::encode_query(text);
    let _ = postguy_urlkit::encode_userinfo(text);
    let _ = postguy_urlkit::decode_percent(text);
    let _ = postguy_urlkit::host_matches(text.split_whitespace().next().unwrap_or_default(), "example.com");
    Ok(())
}

fn vars_target(text: &str) -> Result<(), String> {
    let resolver = postguy_vars::Resolver::new()
        .with_scope(postguy_vars::Scope::new(postguy_vars::ScopeKind::Globals, [("self".into(), text.chars().take(4096).collect::<String>())]))
        .with_scope(postguy_vars::Scope::new(postguy_vars::ScopeKind::Environment, [("chain".into(), "{{self}}{{self||{{self}}}}".into())]));
    let _ = resolver.resolve_text(text);
    Ok(())
}

fn curl_target(text: &str) -> Result<(), String> {
    for parsed in postguy_interop::parse_curl_from_str(text).map_err(|e| format!("{e}"))? {
        let _ = postguy_interop::generate_curl(&parsed.request, 120);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn every_registered_name_dispatches_from_a_good_seed() {
        for name in NAMES {
            let seed = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../fuzz/seeds/{name}/sample.seed"));
            assert!(seed.exists(), "seed missing for {name}: {}", seed.display());
            assert_eq!(run(name, &seed), Ok(()), "{name} should swallow its own seed");
        }
    }

    #[test]
    fn unknown_names_reject_cleanly() {
        assert!(run("not-a-target", Path::new("/dev/null")).is_err());
    }
}
