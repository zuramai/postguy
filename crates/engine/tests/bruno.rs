//! Bruno folder import: the collection, environments (secret values in the secrets store) and
//! the non-request files copied into Assets. Uses the contributor-original interop fixture.

use std::path::{Path, PathBuf};

use postguy_engine::{Engine, EngineError, ImportOptions};
use postguy_model::Id;
use postguy_store::StorePaths;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../interop/tests/fixtures/bruno/sample")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read_dir") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy");
        }
    }
}

fn engine(dir: &Path) -> Engine {
    Engine::open(StorePaths::new(dir.join("workspace"), dir.join("config"))).expect("engine")
}

#[test]
fn imports_collection_environments_and_assets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("sample-src");
    copy_tree(&fixture(), &source);
    std::fs::write(source.join(".env"), "APP_ID=\"abc123\"\nnot valid\n").expect("env");
    std::fs::write(source.join("package.json"), "{}").expect("package");
    std::fs::create_dir_all(source.join("node_modules/x")).expect("mkdir");
    std::fs::write(source.join("node_modules/x/index.js"), "1").expect("node_modules");
    std::fs::write(source.join("environments/notes.txt"), "not an asset").expect("env note");

    let engine = engine(dir.path());
    let outcome = engine.import_bruno(&source, ImportOptions::default()).expect("import");
    assert_eq!(outcome.collection.name, "Sample Shop");
    assert_eq!(outcome.collection.report.environments, 1);
    assert!(outcome.collection.report.requests >= 5, "{:?}", outcome.collection.report);

    assert_eq!(outcome.environments.len(), 1);
    let environment = &outcome.environments[0];
    let id = environment.id.clone().expect("environment id");
    let secrets = engine.workspace().secrets.values_for(&id).expect("secrets");
    assert_eq!(secrets.get("APP_ID").map(String::as_str), Some("abc123"), "{secrets:?}");
    let file = std::fs::read_to_string(engine.store_paths().environment_file(&id).expect("path")).expect("environment file");
    assert!(!file.contains("abc123"), "secret value leaked into the environment file: {file}");
    assert!(file.contains("baseUrl"), "{file}");

    let collection = Id(outcome.collection.id.clone().expect("collection id"));
    let paths: Vec<String> = engine.assets(&collection).expect("assets").into_iter().map(|asset| asset.path).collect();
    assert!(paths.contains(&"lib/utils.js".to_string()), "{paths:?}");
    assert!(paths.contains(&"lib/images/a.png".to_string()), "{paths:?}");
    assert!(!paths.iter().any(|path| path.contains("node_modules") || path.ends_with(".bru") || path.starts_with("environments/")), "{paths:?}");
    assert!(!paths.iter().any(|path| matches!(path.as_str(), "bruno.json" | "package.json")), "{paths:?}");
    assert_eq!(outcome.assets.imported.len(), paths.len());

    let original = std::fs::read(source.join("lib/images/a.png")).expect("png");
    assert_eq!(engine.read_asset(&collection, "lib/images/a.png").expect("read"), original);
}

#[test]
fn collection_name_falls_back_to_the_folder_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("My Suite");
    std::fs::create_dir_all(&source).expect("mkdir");
    std::fs::write(source.join("bruno.json"), r#"{"version":"1","type":"collection"}"#).expect("bruno.json");
    let outcome = engine(dir.path()).import_bruno(&source, ImportOptions::default()).expect("import");
    assert_eq!(outcome.collection.name, "My Suite");
}

#[test]
fn a_directory_without_bruno_json_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("plain");
    std::fs::create_dir_all(&source).expect("mkdir");
    let err = engine(dir.path()).import_bruno(&source, ImportOptions::default()).expect_err("must refuse");
    assert!(matches!(err, EngineError::Import { .. }), "{err:?}");
    assert!(err.to_string().contains("is not a Bruno collection (no bruno.json)"), "{err}");
}
