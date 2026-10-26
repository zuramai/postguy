//! Bruno importer: the Bru parser's edge cases and the folder mapper, against the
//! contributor-original fixture in `tests/fixtures/bruno/sample`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use postguy_interop::bruno::lang::{self, BlockBody};
use postguy_interop::{BrunoFile, BrunoImport, import_bruno};
use postguy_model::auth::AuthLocation;
use postguy_model::{AssertOperator, AuthConfig, Body, Collection, HttpRequest, Item, KeyValueKind, RequestKind};
use proptest::prelude::*;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bruno/sample")
}

fn collect(dir: &Path, prefix: &str, out: &mut Vec<BrunoFile>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).expect("read dir").map(|e| e.expect("entry")).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if entry.path().is_dir() {
            collect(&entry.path(), &path, out);
        } else if name.ends_with(".bru") || name == "bruno.json" {
            out.push(BrunoFile { path, text: std::fs::read_to_string(entry.path()).expect("read") });
        }
    }
}

fn imported() -> BrunoImport {
    let mut files = Vec::new();
    collect(&fixture_root(), "", &mut files);
    let mut dotenv = BTreeMap::new();
    dotenv.insert("API_TOKEN".to_string(), "t0k".to_string());
    import_bruno(&files, &dotenv).expect("import")
}

fn folder<'a>(items: &'a [Item], name: &str) -> &'a postguy_model::Folder {
    items
        .iter()
        .find_map(|item| match item {
            Item::Folder(folder) if folder.name == name => Some(folder.as_ref()),
            _ => None,
        })
        .expect("folder")
}

fn request<'a>(items: &'a [Item], name: &str) -> &'a HttpRequest {
    items
        .iter()
        .find_map(|item| match item {
            Item::Request(request) if request.name == name => match &request.kind {
                RequestKind::Http(http) => Some(http),
                _ => None,
            },
            _ => None,
        })
        .expect("request")
}

fn names(items: &[Item]) -> Vec<&str> {
    items.iter().map(Item::name).collect()
}

fn has(list: &[String], needle: &str) -> bool {
    list.iter().any(|entry| entry.contains(needle))
}

#[test]
fn tree_is_ordered_by_seq_then_name_and_counts_match() {
    let import = imported();
    let collection: &Collection = &import.collection;
    assert_eq!(collection.name, "Sample Shop");
    assert_eq!(names(&collection.items), ["Health", "Orders"]);
    assert_eq!(names(&folder(&collection.items, "Health").items), ["Status", "Ping"]);
    assert_eq!(names(&folder(&collection.items, "Orders").items), ["Create order", "Upload", "Form"]);
    assert_eq!((import.report.requests, import.report.folders, import.report.environments, import.report.collections), (5, 2, 1, 1));
}

#[test]
fn collection_level_files_map_to_the_collection() {
    let import = imported();
    let collection = &import.collection;
    assert!(collection.description.contains("Synthetic sample"));
    assert_eq!(collection.variables.len(), 1);
    assert_eq!((collection.variables[0].key.as_str(), collection.variables[0].value.as_str()), ("region", "eu"));
    match &collection.auth {
        Some(AuthConfig::Bearer(bearer)) => assert_eq!(bearer.token, "{{API_TOKEN}}"),
        other => panic!("expected bearer auth, got {other:?}"),
    }
    let pre = collection.scripts.prerequest.as_ref().expect("root folder.bru script");
    assert!(pre.text.contains("Date.now()"));
}

#[test]
fn assertions_map_operators_and_flag_unknown_ones() {
    let import = imported();
    let orders = folder(&import.collection.items, "Orders");
    let create = request(&orders.items, "Create order");
    let a = &create.tests.assertions;
    assert_eq!(a.len(), 6);
    assert_eq!((a[0].expr.as_str(), a[0].operator, a[0].value.as_str(), a[0].enabled), ("res.status", AssertOperator::Eq, "201", true));
    assert_eq!((a[1].operator, a[1].value.as_str()), (AssertOperator::In, "new, repeat"));
    assert_eq!((a[2].operator, a[2].value.as_str()), (AssertOperator::Neq, "0"));
    assert_eq!((a[3].operator, a[3].value.as_str()), (AssertOperator::IsDefined, ""));
    assert!(!a[4].enabled);
    assert_eq!(a[4].operator, AssertOperator::Eq);
    assert!(!a[5].enabled);
    assert_eq!((a[5].operator, a[5].value.as_str()), (AssertOperator::Eq, "sortOf 3"));
    assert!(has(&import.report.warnings, "unknown operator `sortOf`"));
    let script = create.tests.script.as_ref().expect("tests script");
    assert!(script.text.contains("expect(res.status)"));
}

#[test]
fn create_request_scripts_body_headers_and_auth() {
    let import = imported();
    let orders = folder(&import.collection.items, "Orders");
    let create = request(&orders.items, "Create order");
    let pre = &create.scripts.prerequest.as_ref().expect("pre").text;
    assert!(pre.starts_with("pg.variables.set(\"attempt\", \"1\");\n"), "{pre}");
    assert!(pre.contains("require('./lib/utils')"));
    let post = &create.scripts.test.as_ref().expect("post").text;
    assert!(post.contains("bru.setVar(\"last_order\""));
    assert!(post.contains("pg.setVar(\"order_id\", (res.body.id));"));
    match &create.body {
        Body::Raw { text, .. } => {
            assert!(text.contains("\"note\": \"a } brace\""));
            assert!(text.contains("{{% base64(file('lib/images/a.png')) %}}"));
            assert!(text.starts_with("{\n") && text.ends_with("\n}"));
            assert!(serde_json::from_str::<serde_json::Value>(&text.replace("{{% base64(file('lib/images/a.png')) %}}", "x")).is_ok());
        }
        other => panic!("expected raw body, got {other:?}"),
    }
    match &create.auth {
        AuthConfig::ApiKey(key) => {
            assert_eq!((key.key.as_str(), key.value.as_str(), key.location), ("X-Key", "{{API_KEY}}", AuthLocation::Header));
        }
        other => panic!("expected api key auth, got {other:?}"),
    }
    // The request's own `x-app` wins over the root folder's `X-App`; nothing is duplicated.
    let app: Vec<_> = create.headers.iter().filter(|h| h.key.eq_ignore_ascii_case("x-app")).collect();
    assert_eq!(app.len(), 1);
    assert_eq!(app[0].value, "request-wins");
}

#[test]
fn folder_headers_are_copied_nearest_first_with_a_warning() {
    let import = imported();
    let health = folder(&import.collection.items, "Health");
    let status = request(&health.items, "Status");
    let value = |name: &str| status.headers.iter().find(|h| h.key.eq_ignore_ascii_case(name)).map(|h| (h.value.as_str(), h.enabled));
    assert_eq!(value("X-Beta"), Some(("yes", true)));
    assert_eq!(value("x-app"), Some(("folder-wins", true)));
    assert_eq!(status.headers.iter().filter(|h| h.key.eq_ignore_ascii_case("x-app")).count(), 1);
    assert_eq!(value("X-Skip"), Some(("", false)));
    assert_eq!(value("Empty"), Some(("", true)));
    assert_eq!(status.path_vars.len(), 1);
    assert_eq!(status.path_vars[0].key, "code");
    // A disabled root header is not copied.
    assert_eq!(value("X-Off"), None);
    // The root header reaches requests below folders that do not set it.
    let orders = folder(&import.collection.items, "Orders");
    let upload = request(&orders.items, "Upload");
    assert_eq!(upload.headers.iter().find(|h| h.key == "X-App").map(|h| h.value.as_str()), Some("{{APP_ID}}"));
    assert!(has(&import.report.warnings, "folder.bru: headers have no folder-level field"));
    assert!(has(&import.report.warnings, "beta/folder.bru: headers have no folder-level field"));
}

#[test]
fn folder_level_vars_and_scripts() {
    let import = imported();
    let orders = folder(&import.collection.items, "Orders");
    assert_eq!(orders.variables.len(), 1);
    assert_eq!(orders.variables[0].key, "folderVar");
    assert!(orders.scripts.prerequest.as_ref().expect("script").text.contains("order_scope"));
}

#[test]
fn multipart_file_rows_become_asset_shortcodes() {
    let import = imported();
    let orders = folder(&import.collection.items, "Orders");
    let upload = request(&orders.items, "Upload");
    match &upload.body {
        Body::FormData(rows) => {
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[0].kind, KeyValueKind::Text);
            assert_eq!(rows[1].kind, KeyValueKind::File);
            assert_eq!(rows[1].value, "{{% file('lib/images/a.png') %}}");
            assert!(!rows[2].enabled);
        }
        other => panic!("expected form data, got {other:?}"),
    }
    assert!(has(&import.report.warnings, "import `lib/images/a.png` into Assets"));
}

#[test]
fn urlencoded_unknown_auth_and_dropped_entries() {
    let import = imported();
    let orders = folder(&import.collection.items, "Orders");
    let form = request(&orders.items, "Form");
    match &form.body {
        Body::UrlEncoded(rows) => {
            assert_eq!(rows[0].value, "{{ACCOUNT}}");
            assert_eq!(rows[0].description, "Account id");
            assert!(!rows[1].enabled);
        }
        other => panic!("expected urlencoded, got {other:?}"),
    }
    assert!(matches!(&form.auth, AuthConfig::Unknown { kind, .. } if kind == "oauth2"));
    assert!(has(&import.report.dropped, "auth mode `oauth2` is not supported"));
    assert!(has(&import.report.dropped, "alpha/stream.bru: request type `grpc` is not supported"));
    assert!(has(&import.report.dropped, "beta/ping.bru: settings: not supported"));
}

#[test]
fn npm_requires_are_warned_but_builtins_and_relative_are_not() {
    let import = imported();
    assert!(has(&import.report.warnings, "alpha/create.bru: requires npm package `axios`, which Postguy cannot load"));
    assert!(!has(&import.report.warnings, "package `fs`"));
    assert!(!has(&import.report.warnings, "package `./lib/utils`"));
}

#[test]
fn process_env_is_rewritten_and_becomes_secrets_in_every_environment() {
    let import = imported();
    let expected: Vec<&str> = vec!["ACCOUNT", "API_KEY", "API_TOKEN", "APP_ID"];
    assert_eq!(import.process_env.iter().map(String::as_str).collect::<Vec<_>>(), expected);
    let serialized = serde_json::to_string(&import.collection).expect("json");
    assert!(!serialized.contains("process.env"));
    assert_eq!(import.environments.len(), 1);
    let dev = &import.environments[0];
    assert_eq!(dev.name, "dev");
    let get = |key: &str| dev.variables.iter().find(|v| v.key == key).expect("variable");
    assert_eq!(get("baseUrl").value, "http://localhost:8080");
    assert!(!get("baseUrl").secret);
    assert!(!get("unused").enabled);
    assert!(get("API_KEY").secret && get("API_KEY").value.is_empty());
    assert_eq!(dev.variables.iter().filter(|v| v.key == "API_KEY").count(), 1);
    assert!(get("token").secret);
    assert!(get("API_TOKEN").secret);
    assert_eq!(get("API_TOKEN").value, "t0k");
    assert!(get("APP_ID").secret && get("APP_ID").value.is_empty());
    assert!(has(&import.report.warnings, "no value for process.env names: ACCOUNT, API_KEY, APP_ID"));
}

#[test]
fn a_file_that_fails_to_parse_is_dropped_not_fatal() {
    let files = vec![
        BrunoFile { path: "bruno.json".into(), text: "{\"name\":\"X\"}".into() },
        BrunoFile { path: "bad.bru".into(), text: "meta {\n  name: Bad\n".into() },
        BrunoFile { path: "ok.bru".into(), text: "meta {\n  name: Ok\n}\n\nget {\n  url: http://x\n}\n".into() },
    ];
    let import = import_bruno(&files, &BTreeMap::new()).expect("import");
    assert_eq!(import.report.requests, 1);
    assert!(has(&import.report.dropped, "bad.bru"));
}

#[test]
fn parser_edge_cases() {
    let text = "meta {\n  name: Edge\n  ~seq: 4\n  empty:\n  url: http://a/b?c=d: e\n  \"quoted key\": v\n}\n\nbody:json {\n  {\n    \"s\": \"}\",\n    \"t\": [\n      1\n    ]\n  }\n}\n\nheaders {\n  @description('''first\n  second''')\n  A: '''x\n    y\n  '''\n  B: 1\n}\n\nvars:secret [ one, ~two ]\n";
    let doc = lang::parse(text).expect("parse");
    let meta = doc.dict("meta").expect("meta");
    assert_eq!(meta.len(), 5);
    assert_eq!((meta[0].key.as_str(), meta[0].value.as_str(), meta[0].enabled), ("name", "Edge", true));
    assert_eq!((meta[1].key.as_str(), meta[1].enabled), ("seq", false));
    assert_eq!((meta[2].key.as_str(), meta[2].value.as_str()), ("empty", ""));
    assert_eq!(meta[3].value, "http://a/b?c=d: e");
    assert_eq!(meta[4].key, "quoted key");
    // An indented `}` and `]` inside a text block do not end it.
    assert_eq!(doc.text("body:json"), Some("{\n  \"s\": \"}\",\n  \"t\": [\n    1\n  ]\n}"));
    let headers = doc.dict("headers").expect("headers");
    assert_eq!(headers[0].description.as_deref(), Some("first\n  second"));
    assert_eq!(headers[0].value, "x\ny");
    assert_eq!(headers[1].value, "1");
    assert_eq!(doc.list("vars:secret"), Some(&["one".to_string(), "~two".to_string()][..]));
    assert!(matches!(doc.block("meta").map(|b| &b.body), Some(BlockBody::Dict(_))));
}

#[test]
fn parser_caps_size_and_block_count() {
    assert!(lang::parse(&"x".repeat(lang::MAX_FILE_BYTES + 1)).is_err());
    let many = "a {\n}\n".repeat(lang::MAX_BLOCKS + 1);
    assert!(lang::parse(&many).is_err());
}

#[test]
fn postguy_tests_round_trip_through_postman_and_leave_extras() {
    let import = imported();
    let exported = postguy_interop::export_collection(&import.collection);
    let text = serde_json::to_string(&exported).expect("json");
    assert!(text.contains("postguyTests"));
    let (imported_back, _) = postguy_interop::import_value(&exported).expect("re-import");
    let postguy_interop::Imported::Collection(back) = imported_back else { panic!("expected a collection") };
    let orders = folder(&back.items, "Orders");
    let create = request(&orders.items, "Create order");
    let original = request(&folder(&import.collection.items, "Orders").items, "Create order");
    assert_eq!(create.tests, original.tests);
    assert_eq!(create.tests.assertions.len(), 6);
    let Some(Item::Request(item)) = orders.items.iter().find(|i| i.name() == "Create order") else { panic!("request item") };
    assert!(item.extras.get("postguyTests").is_none());
    assert!(!serde_json::to_string(&item.extras).expect("json").contains("postguyTests"));
    // A request without tests writes no key at all.
    let health = folder(&import.collection.items, "Health");
    let _ = request(&health.items, "Ping");
    let again = postguy_interop::export_collection(&back);
    assert_eq!(serde_json::to_string(&again).expect("json").matches("postguyTests").count(), 1);
}

proptest! {
    #[test]
    fn parse_never_panics(text in ".{0,400}") {
        let _ = lang::parse(&text);
    }

    #[test]
    fn parse_never_panics_on_block_shaped_input(parts in proptest::collection::vec("[a-z:\\-~@'(){}\\[\\], \\n]{0,20}", 0..30)) {
        let _ = lang::parse(&parts.join("\n"));
    }
}
