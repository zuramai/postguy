//! Fuzz targets for `cargo xtask fuzz` : each is a pure parser entry the fuzzer's
//! child process calls with a mutated seed. No network, no filesystem writes; a panic here is
//! a BUG per AGENTS.md 4 and the violated `#![deny]` contract defended below.
//!
//! Targets registered in `xtask/src/fuzz.rs`'s `TARGETS` map to these names; each takes the
//! seed PATH (already size-capped by the driver) and swallows all outcomes — the point is the
//! process's survival, not the parse result.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Registered target names (keeps the xtask table and this dispatch one source).
pub const NAMES: &[&str] = &["url", "vars", "curl", "script", "data", "openapi"];

/// Run one target over one file; `Ok(())` = the crate survived. Any panic escapes and the
/// driver records a finding.
pub fn run(name: &str, file: &std::path::Path) -> Result<(), String> {
    let bytes = read_capped(file)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    match name {
        "url" => url_target(&text),
        "vars" => vars_target(&text),
        "curl" => curl_target(&text),
        "script" => script_target(&text),
        "data" => data_target(&bytes),
        "openapi" => openapi_target(&text),
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

/// Both parsers get every input: the file name only picks the format for ambiguous bytes.
fn data_target(bytes: &[u8]) -> Result<(), String> {
    let _ = postguy_runner::parse_data(bytes, "x.csv");
    let _ = postguy_runner::parse_data(bytes, "x.json");
    Ok(())
}

/// YAML and JSON both go through the budgeted reader, then the importer in both layouts.
fn openapi_target(text: &str) -> Result<(), String> {
    let parsed = postguy_interop::parse_yaml(text).ok().or_else(|| serde_json::from_str(text).ok());
    if let Some(value) = parsed {
        for strategy in [postguy_interop::FolderStrategy::Tags, postguy_interop::FolderStrategy::Paths] {
            let _ = postguy_interop::import_openapi(&value, strategy);
        }
    }
    Ok(())
}

struct NoHost;

impl postguy_script::Host for NoHost {
    fn send_request(&self, _: postguy_script::ScriptRequest) -> Result<postguy_script::ScriptResponse, String> {
        Err("no network in the fuzz target".to_string())
    }
    fn cookies_for(&self, _: &str) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// Run the input as a post-response script under tight limits; the sandbox must return.
fn script_target(text: &str) -> Result<(), String> {
    let context = postguy_script::Context {
        phase: postguy_model::ScriptPhase::PostResponse,
        info: postguy_script::Info { event_name: "test", iteration: 0, iteration_count: 1, request_name: "fuzz".into(), request_id: String::new() },
        variables: postguy_script::Variables::default(),
        request: postguy_script::ScriptRequest { method: "GET".into(), url: "http://localhost/".into(), ..Default::default() },
        response: Some(postguy_script::ScriptResponse { code: 200, status: "OK".into(), body: br#"{"a":1}"#.to_vec(), ..Default::default() }),
        host: std::sync::Arc::new(NoHost),
    };
    let limits = postguy_script::Limits { timeout_ms: 200, memory_bytes: 16 << 20, ..Default::default() };
    let source = postguy_script::ScriptSource { origin: "fuzz".into(), text: text.to_string() };
    let _ = postguy_script::run(&[source], context, &limits, std::sync::Arc::new(|| false));
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
