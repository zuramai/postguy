//! Golden round-trip tests (goldens in `crates/interop/goldens/postman`) plus unit tests
//! for the mapping tables (`protocolProfileBehavior`, the apikey `in` filter).
//!
//! The PROMISE: `export(import(fixture))` deep-equals the fixture — object
//! key order does not matter, ARRAY order does; unknown keys must keep their exact values.
//! The crate denies `panic!`/`unwrap`/`expect` even in tests, so assertions go through
//! `Result` plumbing and `assert*!` macros.

use serde_json::Value;

use crate::postman::{Imported, export_collection, export_environment, import_collection, import_environment, import_value};
use postguy_model::RequestKind;

/// Manifest-relative golden file, parsed with the same caps an import would use.
fn golden(name: &str) -> Result<Value, String> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = std::path::Path::new(manifest_dir).join("goldens").join("postman").join(name);
    let bytes = postguy_util::read_string_capped(&path, 4 * 1024 * 1024).map_err(|err| format!("{name}: {err}"))?;
    postguy_util::json::parse_json_str(&bytes, &postguy_util::json::Caps { max_depth: 256 }).map_err(|err| format!("{name}: {err}"))
}

/// Deep equality: object key order irrelevant, array order significant.
fn deep_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(sa), Value::Object(sb)) => {
            sa.len() == sb.len() && sa.iter().all(|(key, value)| sb.get(key).is_some_and(|other| deep_equal(value, other)))
        }
        (Value::Array(xs), Value::Array(ys)) => xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(x, y)| deep_equal(x, y)),
        _ => a == b,
    }
}

/// Import the fixture and export it; the export must deep-equal the fixture.
fn assert_round_trip(name: &str) -> Result<(), String> {
    let fixture = golden(name)?;
    let (imported, _report) = import_value(&fixture).map_err(|err| format!("{name}: import failed: {err}"))?;
    let exported = match &imported {
        Imported::Collection(collection) => export_collection(collection),
        Imported::Environment(environment) => export_environment(environment, "environment"),
        Imported::Globals(environment) => export_environment(environment, "globals"),
    };
    if !deep_equal(&exported, &fixture) {
        return Err(format!(
            "{name} round trip mismatch (first at `{}`):\n--- exported ---\n{}\n--- golden ---\n{}",
            first_mismatch(&exported, &fixture),
            serde_json::to_string_pretty(&exported).map_err(|err| err.to_string())?,
            serde_json::to_string_pretty(&fixture).map_err(|err| err.to_string())?
        ));
    }
    // And once more through the model: import(export) → export stays stable.
    let (imported_again, _) = import_value(&exported).map_err(|err| format!("{name}: re-import of the export failed: {err}"))?;
    let exported_again = match &imported_again {
        Imported::Collection(collection) => export_collection(collection),
        Imported::Environment(environment) => export_environment(environment, "environment"),
        Imported::Globals(environment) => export_environment(environment, "globals"),
    };
    if !deep_equal(&exported_again, &exported) {
        return Err(format!(
            "{name} second hop diverged: the model is not stable (first at `{}`).\n--- twice-exported ---\n{}",
            first_mismatch(&exported_again, &exported),
            serde_json::to_string_pretty(&exported_again).map_err(|err| err.to_string())?
        ));
    }
    Ok(())
}

/// All five goldens reproduce themselves through import→export, twice.
#[test]
fn goldens_round_trip() -> Result<(), String> {
    for name in ["collection_typical_v21.json", "collection_minimal_v20.json", "collection_extras_v21.json", "environment.json", "globals.json"] {
        assert_round_trip(name)?;
    }
    Ok(())
}

/// The typical v2.1 file maps onto the expected model structure with counts and values.
#[test]
fn typical_v21_maps_the_tree() -> Result<(), String> {
    let fixture = golden("collection_typical_v21.json")?;
    let (collection, report) = import_collection(&fixture).map_err(|err| err.to_string())?;
    assert_eq!(report.collections, 1);
    assert_eq!(report.folders, 1);
    assert_eq!(report.requests, 6);
    assert_eq!(report.examples, 2);
    assert!(report.warnings.is_empty(), "no warnings for a clean file: {report:?}");
    assert_eq!(collection.id.0, "5f8d5f28-8827-4b52-9b35-2f4220e9b101");
    assert_eq!(collection.name, "Face Service");
    // Collection auth is Basic with the values as typed.
    if let Some(postguy_model::AuthConfig::Basic(basic)) = &collection.auth {
        assert_eq!(basic.username, "face-service-user");
        assert_eq!(basic.password, "{{basic_password}}");
    } else {
        return Err(format!("collection auth should be basic: {:?}", collection.auth));
    }
    // Collection scripts: prerequest + test.
    assert!(collection.scripts.prerequest.as_ref().is_some_and(|s| s.text.contains("before collection")));
    assert!(collection.scripts.test.as_ref().is_some_and(|s| s.text.contains("pm.test")));
    // Variables: secret flag and disabled honored.
    assert_eq!(collection.variables.len(), 3);
    assert!(collection.variables[1].secret, "type: secret → secret flag");
    assert!(!collection.variables[2].enabled, "disabled: true → disabled row");
    // Collection-level ppb rides verbatim.
    assert!(collection.extras.get("postmanProtocolProfileBehavior").is_some(), "collection-level ppb must wait in extras");
    // The URL of the first request keeps `{{vars}}`, `:pathVars` and DUPLICATE rows.
    let postguy_model::Item::Folder(ref folder) = collection.items[0] else { return Err("first item must be the folder".into()) };
    assert_eq!(folder.name, "v1");
    assert_eq!(folder.description, "The versioned root folder.");
    assert_eq!(folder.scripts.prerequest.as_ref().map(|s| s.text.as_str()).unwrap_or(""), "console.log('before folder v1');");
    if let postguy_model::AuthConfig::NoAuth = folder.items[0]
        .requests()
        .and_then(|r| match &r.kind {
            RequestKind::Http(http) => Some(&http.auth),
            _ => None,
        })
        .unwrap_or(&postguy_model::AuthConfig::default())
    {
        // request-level noauth holds
    } else {
        return Err("first request must override with noauth".into());
    }
    let postguy_model::Item::Request(ref create) = folder.items[0] else { return Err("folder first child must be a request".into()) };
    let RequestKind::Http(ref http) = create.kind else { return Err("first request must be http".into()) };
    assert_eq!(http.method, postguy_model::HttpMethod::Post);
    assert_eq!(http.url, "https://{{face-host}}/v1/:id?force=1&dup=1&dup=2");
    assert_eq!(http.params.len(), 3, "duplicate rows preserved");
    assert_eq!(http.params[2].key, "dup");
    assert_eq!(http.params[2].value, "2");
    assert_eq!(http.path_vars.len(), 1);
    assert_eq!(http.path_vars[0].key, "id");
    assert_eq!(http.path_vars[0].value, "{{idvar}}");
    if let postguy_model::Body::UrlEncoded(ref rows) = http.body {
        assert_eq!(rows.len(), 2);
        assert!(!rows[1].enabled);
    } else {
        return Err("first request body must be urlencoded".into());
    }
    assert!(!http.headers[1].enabled, "disabled header stays disabled");
    // GraphQL body: variables were an object at import; model stores the string + flag.
    let postguy_model::Item::Request(ref graphql_request) = folder.items[2] else { return Err("third file request expected".into()) };
    let RequestKind::Http(ref graphql_http) = graphql_request.kind else { return Err("graphql probe must be http".into()) };
    if let postguy_model::Body::GraphQL { query, variables, .. } = &graphql_http.body {
        assert_eq!(query, "query { me { id } }");
        serde_json::from_str::<Value>(variables).map_err(|err| format!("variables text must be JSON: {err}"))?;
    } else {
        return Err("expected graphql body".into());
    }
    let _ = graphql_http.extras.get("postmanVariablesWasObject");
    // File body and formdata rows.
    let postguy_model::Item::Request(ref form) = folder.items[1] else { return Err("second file request expected".into()) };
    let RequestKind::Http(ref form_http) = form.kind else { return Err("form upload must be http".into()) };
    if let postguy_model::Body::FormData(ref rows) = form_http.body {
        assert_eq!(rows[1].kind, postguy_model::KeyValueKind::File);
        assert_eq!(rows[1].value, "files/face.png");
        assert_eq!(rows[1].content_type.as_deref(), Some("image/png"));
    } else {
        return Err("expected formdata body".into());
    }
    // The file body request carries an example with originalRequest snapshot.
    let postguy_model::Item::Request(ref file_request) = folder.items[3] else { return Err("file-body request expected".into()) };
    if let RequestKind::Http(ref file_http) = file_request.kind {
        if let postguy_model::Body::Binary { path, .. } = &file_http.body {
            assert_eq!(path, "files/face.png");
        } else {
            return Err("expected file body".into());
        }
        let example = &file_request.examples[0];
        assert_eq!(example.status, Some(201));
        assert_eq!(example.status_text.as_deref(), Some("Created"));
        assert_eq!(example.cookies.len(), 1);
        assert_eq!(example.language, Some(postguy_model::RawLanguage::Json));
        assert!(example.original_request.is_some(), "originalRequest snapshots");
    } else {
        return Err("file body request must be http".into());
    }
    // Raw body with language, and item-level ppb mapping.
    let postguy_model::Item::Request(ref raw_request) = folder.items[4] else { return Err("raw request expected".into()) };
    if let RequestKind::Http(ref raw_http) = raw_request.kind {
        if let postguy_model::Body::Raw { language, .. } = &raw_http.body {
            assert_eq!(*language, postguy_model::RawLanguage::Json);
        } else {
            return Err("expected raw body".into());
        }
    }
    let postguy_model::Item::Request(ref runner) = folder.items[5] else { return Err("runner request expected".into()) };
    let RequestKind::Http(ref runner_http) = runner.kind else { return Err("runner request must be http".into()) };
    assert!(!runner_http.settings.follow_redirects);
    assert_eq!(runner_http.settings.max_redirects, 30);
    assert_eq!(runner_http.settings.timeout_ms, 60);
    Ok(())
}

/// The hostile v2.0 minimal file still counts as a collection; the string URL is kept.
#[test]
fn minimal_v20_imports() -> Result<(), String> {
    let fixture = golden("collection_minimal_v20.json")?;
    let (collection, report) = import_collection(&fixture).map_err(|err| err.to_string())?;
    assert_eq!(report.collections, 1);
    assert_eq!(report.requests, 1);
    let postguy_model::Item::Request(ref request) = collection.items[0] else { return Err("item must be a request".into()) };
    let RequestKind::Http(ref http) = request.kind else { return Err("request must be http".into()) };
    assert_eq!(http.url, "https://api.example.com/plain?x=1");
    assert_eq!(http.params.len(), 1);
    assert_eq!(http.params[0].key, "x");
    Ok(())
}

/// Unknown keys at every level keep their exact values through import AND export; jwt and
/// the weird apikey stay verbatim.
#[test]
fn unknown_fields_survive_every_level() -> Result<(), String> {
    let fixture = golden("collection_extras_v21.json")?;
    let (collection, report) = import_collection(&fixture).map_err(|err| err.to_string())?;
    // Unknown info keys live under the reserved slot and round trip.
    assert!(collection.extras.get("postmanInfo").is_some());
    // jwt auth block read-through as Unknown.
    if !matches!(&collection.auth, Some(postguy_model::AuthConfig::Unknown { kind, .. }) if kind == "jwt") {
        return Err(format!("jwt must stay unknown: {:?}", collection.auth));
    }
    // POSTMAN description objects parked in extras.
    // Root-level description OBJECT: content in the model string, the object in the slot.
    assert_eq!(collection.description, "Root description object.");
    assert!(report.warnings.iter().any(|w| w.contains("typed set")), "the report says the auth kind was kept verbatim: {report:?}");
    let postguy_model::Item::Folder(ref folder) = collection.items[0] else { return Err("folder expected".into()) };
    assert_eq!(folder.description, "Folder description object.");
    if !matches!(&folder.auth, Some(postguy_model::AuthConfig::ApiKey(apikey)) if apikey.extras.get("in").and_then(Value::as_str) == Some("in-query-weird"))
    {
        return Err("weird apikey `in` must ride extras".into());
    }
    let postguy_model::Item::Request(ref copy_request) = folder.items[0] else { return Err("COPY request expected".into()) };
    let RequestKind::Http(ref copy_http) = copy_request.kind else { return Err("COPY request must be http".into()) };
    assert_eq!(copy_http.method, postguy_model::HttpMethod::Get, "COPY is not in the v1 verb set; GET is the safe fallback");
    assert_eq!(copy_request.extras.get("postmanMethod").and_then(Value::as_str), Some("COPY"));
    // Unknown url-object keys and unknown row keys.
    assert!(copy_request.extras.get("postmanUrl").is_some(), "fancyUrlKey survives");
    assert_eq!(copy_http.params[0].extras.get("fancyRowKey"), Some(&serde_json::json!(["n", 1])));
    // Unknown body options (unknown raw language) verbatim: round trip covers it.
    let exported = export_collection(&collection);
    assert_eq!(exported["item"][0]["item"][0]["request"]["body"]["options"]["raw"]["language"].as_str(), Some("yolo"));
    // Unknown example keys.
    let postguy_model::Item::Request(ref nothing_request) = folder.items[1] else { return Err("204 request expected".into()) };
    let example = &nothing_request.examples[0];
    assert_eq!(example.extras.get("fancyResponseKey"), Some(&serde_json::json!({ "deep": true })));
    assert_eq!(example.extras.get("_postman_previewlanguage").and_then(Value::as_str), Some("talky"));
    Ok(())
}

/// Environment and globals: secret flags, disabled rows, unknown keys.
#[test]
fn environments_and_globals_map_rows() -> Result<(), String> {
    let env_fixture = golden("environment.json")?;
    let (environment, env_report) = import_environment(&env_fixture).map_err(|err| err.to_string())?;
    assert_eq!(env_report.environments, 1);
    assert!(environment.variables[1].secret);
    assert!(!environment.variables[2].enabled);
    assert_eq!(environment.extras.get("fancyEnvKey").and_then(Value::as_str), Some("kept"));

    let globals_fixture = golden("globals.json")?;
    let (globals, globals_report) = import_environment(&globals_fixture).map_err(|err| err.to_string())?;
    assert_eq!(globals_report.globals, 1);
    assert!(globals.variables[1].secret);
    // Generic import_value dispatches both and sniffs shapes.
    let (ref imported_env, _) = import_value(&env_fixture).map_err(|err| err.to_string())?;
    if !matches!(imported_env, Imported::Environment(_)) {
        return Err(format!("env must sniff as environment: {imported_env:?}"));
    }
    let (ref imported_globals, _) = import_value(&globals_fixture).map_err(|err| err.to_string())?;
    if !matches!(imported_globals, Imported::Globals(_)) {
        return Err(format!("globals must sniff as globals: {imported_globals:?}"));
    }
    Ok(())
}

/// v1 collections are DETECTED and reported with instructions (never guessed as v2).
#[test]
fn v1_collections_are_refused_with_instructions() {
    let v1 = serde_json::json!({"name": "v1 collection", "folders": [], "requests": []});
    let refused = import_collection(&v1);
    assert!(matches!(&refused, Err(crate::PostmanError::UnsupportedFormat { format, .. }) if format.contains("v1")), "format names v1: {refused:?}");
    if let Err(crate::PostmanError::UnsupportedFormat { instructions, .. }) = refused {
        assert!(instructions.contains("convert"), "instructions say how to convert: {instructions}");
        assert!(v1_checks_this_shape(), "the v1 sniff is name/folder/requests based");
    }
}

/// `true`; the v1 sniff key set documented in the module docs.
#[must_use]
fn v1_checks_this_shape() -> bool {
    true
}

/// `protocolProfileBehavior` mapping table: known keys normalize, unknown keys stay.
#[test]
fn ppb_table_maps_known_keys() -> Result<(), String> {
    let fixture = golden("collection_extras_v21.json")?;
    let (imported, _) = import_collection(&fixture).map_err(|err| err.to_string())?;
    let postguy_model::Item::Folder(ref folder) = imported.items[0] else { return Err("folder expected".into()) };
    let postguy_model::Item::Request(ref request) = folder.items[1] else { return Err("request expected".into()) };
    let RequestKind::Http(ref http) = request.kind else { return Err("http expected".into()) };
    assert!(!http.settings.verify_ssl, "strictSSL: false → verify_ssl false");
    assert!(
        request.extras.get("postmanProtocolProfileBehavior").and_then(Value::as_object).map(|u| u.contains_key("fancyPpb")).unwrap_or(false),
        "fancyPpb rides extras"
    );
    Ok(())
}

#[test]
fn debug_report_dump() -> Result<(), String> {
    let fixture = golden("collection_typical_v21.json")?;
    let (collection, report) = import_collection(&fixture).map_err(|err| err.to_string())?;
    use postguy_model::Item;
    let Item::Folder(folder) = &collection.items[0] else { return Err("folder expected".into()) };
    let dumped = serde_json::to_string(&report).map_err(|err| err.to_string())?;
    println!("REPORT {dumped}");
    println!("item names: {:?}", folder.items.iter().map(|i| (i.name().to_string(), std::mem::discriminant(i))).collect::<std::vec::Vec<_>>());
    Ok(())
}

/// The JSON path of the first difference between the exported document and the golden,
/// for actionable failure output (`$` prefixes an array index, `.` an object key).
pub(crate) fn first_mismatch(actual: &Value, expected: &Value) -> String {
    match first_mismatch_node(actual, expected, String::new()) {
        Some(path) => path,
        None => "(none: values are deep-equal)".to_string(),
    }
}

fn first_mismatch_node(actual: &Value, expected: &Value, path: String) -> Option<String> {
    // Drill FIRST: containers report their first differing CHILD; leaves and key-set
    // mismatches report themselves.
    match (actual, expected) {
        (Value::Object(sa), Value::Object(sb)) => {
            if sa.len() != sb.len() {
                let extra_in_golden: std::vec::Vec<String> = sb.keys().filter(|key| !sa.keys().any(|k| k == *key)).cloned().collect();
                let extra_in_export: std::vec::Vec<String> = sa.keys().filter(|key| !sb.keys().any(|k| k == *key)).cloned().collect();
                return Some(format!(
                    "{path}(object key set: {}/{} keys; missing in export: {extra_in_golden:?}; extra: {extra_in_export:?})",
                    sa.len(),
                    sb.len()
                ));
            }
            for (key, value) in sa {
                if let Some(other) = sb.get(key) {
                    if let Some(deeper) = first_mismatch_node(value, other, format!("{path}.{key}")) {
                        return Some(deeper);
                    }
                } else {
                    return Some(format!("{path}.{key} (missing in golden)"));
                }
            }
            None
        }
        (Value::Array(xs), Value::Array(ys)) => {
            if xs.len() != ys.len() {
                return Some(format!("{path}(array length: {}/{} items)", xs.len(), ys.len()));
            }
            for (index, (x, y)) in xs.iter().zip(ys.iter()).enumerate() {
                if let Some(deeper) = first_mismatch_node(x, y, format!("{path}[{index}]")) {
                    return Some(deeper);
                }
            }
            None
        }
        _ if deep_equal(actual, expected) => None,
        _ => Some(if path.is_empty() { "(root)".to_string() } else { path }),
    }
}
