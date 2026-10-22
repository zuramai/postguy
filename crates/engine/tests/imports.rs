//! Import auto-detection, name-clash handling and explicit replacement plus folder
//! walking with caps. The files are synthetic and written by the test.

use postguy_engine::{Engine, ImportKind, ImportOptions, detect, walk_import_paths};
use postguy_store::StorePaths;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture { dir: tempfile::tempdir().expect("tempdir") }
    }

    fn engine(&self) -> Engine {
        Engine::open(StorePaths::new(self.dir.path().join("workspace"), self.dir.path().join("config"))).expect("engine")
    }

    fn write(&self, name: &str, text: &str) -> std::path::PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, text).expect("write fixture");
        path
    }
}

fn collection_json(name: &str) -> String {
    format!(
        r#"{{"info":{{"_postman_id":"11111111-1111-4111-8111-111111111111","name":"{name}","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"}},"item":[{{"name":"Echo","request":{{"method":"GET","url":"https://example.test/echo"}}}}]}}"#
    )
}

fn environment_json(name: &str) -> String {
    format!(
        r#"{{"name":"{name}","values":[{{"key":"base","value":"https://example.test","enabled":true}}],"_postman_variable_scope":"environment"}}"#
    )
}

fn globals_json() -> &'static str {
    r#"{"name":"Globals","values":[{"key":"who","value":"globals","enabled":true}],"_postman_variable_scope":"globals"}"#
}

#[test]
fn collection_name_and_id_clashes_get_a_suffix_and_a_fresh_id() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let first = engine.import_text(&collection_json("Payments"), ImportOptions::default()).expect("first import");
    assert_eq!(first.kind, ImportKind::Collection);
    assert_eq!(first.name, "Payments");
    let first_id = first.id.clone().expect("collection id");
    let second = engine.import_text(&collection_json("Payments"), ImportOptions::default()).expect("second import");
    assert_eq!(second.name, "Payments (2)");
    assert_ne!(second.id.as_deref(), Some(first_id.as_str()), "the clashing id was replaced");
    assert!(second.report.warnings.iter().any(|warning| warning.contains("name clash")));
    assert_eq!(engine.collections().expect("collections").len(), 2);
}

#[test]
fn replacing_the_same_id_keeps_the_name_and_does_not_duplicate() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let first = engine.import_text(&collection_json("Payments"), ImportOptions::default()).expect("first");
    let id = first.id.clone().expect("id");
    let replaced = engine.import_text(&collection_json("Payments"), ImportOptions { replace: true }).expect("replace");
    assert_eq!(replaced.id.as_deref(), Some(id.as_str()));
    assert_eq!(replaced.name, "Payments");
    assert_eq!(engine.collections().expect("collections").len(), 1);
}

#[test]
fn environment_clashes_are_suffixed_and_replaceable() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let first = engine.import_text(&environment_json("Staging"), ImportOptions::default()).expect("first");
    assert_eq!(first.kind, ImportKind::Environment);
    let second = engine.import_text(&environment_json("Staging"), ImportOptions::default()).expect("second");
    assert_eq!(second.name, "Staging (2)");
    assert_eq!(engine.environments().expect("environments").len(), 2);
    let replaced = engine.import_text(&environment_json("Staging"), ImportOptions { replace: true }).expect("replace");
    assert_eq!(replaced.id.as_deref(), first.id.as_deref());
}

#[test]
fn globals_require_an_explicit_replace() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let first = engine.import_text(globals_json(), ImportOptions::default()).expect("first");
    assert_eq!(first.kind, ImportKind::Globals);
    let error = engine.import_text(globals_json(), ImportOptions::default()).expect_err("refused");
    assert!(error.to_string().contains("Replace existing"), "{error}");
    engine.import_text(globals_json(), ImportOptions { replace: true }).expect("replace");
}

#[test]
fn curl_text_imports_one_collection_with_named_requests() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let text = "curl -X POST 'https://example.test/v1/charges' -H 'Content-Type: application/json' -d '{\"amount\":1}'\ncurl https://example.test/v1/payments?limit=2";
    let outcome = engine.import_text(text, ImportOptions::default()).expect("curl import");
    assert_eq!(outcome.kind, ImportKind::Collection);
    assert_eq!(outcome.report.requests, 2);
    let collection = engine.collection(&postguy_model::Id(outcome.id.expect("id"))).expect("collection");
    let names: Vec<&str> = collection
        .items
        .iter()
        .filter_map(|item| match item {
            postguy_model::Item::Request(request) => Some(request.name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, ["POST charges", "GET payments"]);
}

#[test]
fn urls_and_empty_text_are_refused_with_guidance() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    assert_eq!(detect("https://example.test/a.json"), postguy_engine::Detected::Url);
    let url = engine.import_text("https://example.test/a.json", ImportOptions::default()).expect_err("url");
    assert!(url.to_string().contains("download the file"), "{url}");
    let empty = engine.import_text("   ", ImportOptions::default()).expect_err("empty");
    assert!(empty.to_string().contains("nothing to import"), "{empty}");
}

#[test]
fn import_file_reads_capped_utf8_and_folder_walks_find_nested_files() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let path = fixture.write("collection.json", &collection_json("From file"));
    let outcome = engine.import_file(&path, ImportOptions::default()).expect("file import");
    assert_eq!(outcome.name, "From file");

    let folder = fixture.dir.path().join("dump");
    std::fs::create_dir_all(folder.join("nested")).expect("folder");
    std::fs::write(folder.join("a.json"), collection_json("A")).expect("file a");
    std::fs::write(folder.join("nested/b.json"), collection_json("B")).expect("file b");
    std::fs::write(folder.join("note.txt"), "not json or curl").expect("file txt");
    let (files, problems) = walk_import_paths([folder]);
    assert_eq!(files.len(), 3);
    assert!(problems.is_empty(), "{problems:?}");
    let names: Vec<String> = files.iter().map(|path| path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_string()).collect();
    assert!(names.contains(&"a.json".to_string()) && names.contains(&"b.json".to_string()));
}
