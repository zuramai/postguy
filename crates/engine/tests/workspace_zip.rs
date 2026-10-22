//! All-data zip export and zip import: documents travel, `.postguy/` state and `.bak`
//! files do not, and hostile paths never extract to disk.

use std::io::Write;

use postguy_engine::{Engine, ImportOptions};
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

    fn workspace(&self) -> std::path::PathBuf {
        self.dir.path().join("workspace")
    }
}

fn collection_json(name: &str) -> String {
    format!(r#"{{"info":{{"name":"{name}","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"}},"item":[]}}"#)
}

#[test]
fn export_includes_documents_but_not_secrets_or_backups() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    engine.create_collection("Exported").expect("collection");
    let workspace = fixture.workspace();
    std::fs::create_dir_all(workspace.join("files")).expect("files");
    std::fs::write(workspace.join("files/example.bin"), b"payload").expect("file");
    std::fs::create_dir_all(workspace.join(".postguy")).expect("state");
    std::fs::write(workspace.join(".postguy/secrets.json"), b"{\"token\":\"hunter2\"}").expect("secrets");
    std::fs::write(workspace.join("collections/old.json.bak"), b"{}").expect("backup");
    let out = fixture.dir.path().join("workspace.zip");
    engine.export_workspace_zip(&out).expect("export");
    let file = std::fs::File::open(&out).expect("zip");
    let mut archive = zip::ZipArchive::new(file).expect("archive");
    let mut names = Vec::new();
    for index in 0..archive.len() {
        names.push(archive.by_index(index).expect("entry").name().expect("name").to_string());
    }
    assert!(names.iter().any(|name| name.starts_with("collections/") && name.ends_with(".json")));
    assert!(names.iter().any(|name| name == "files/example.bin"));
    assert!(!names.iter().any(|name| name.contains(".postguy")), "{names:?}");
    assert!(!names.iter().any(|name| name.ends_with(".bak")), "{names:?}");
    assert!(names.iter().any(|name| name == "postguy.json"));
}

#[test]
fn zip_import_reads_json_entries_and_refuses_traversal() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let zip_path = fixture.dir.path().join("incoming.zip");
    let file = std::fs::File::create(&zip_path).expect("zip file");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("nested/collection.json", options).expect("entry");
    writer.write_all(collection_json("From zip").as_bytes()).expect("write");
    writer.start_file("../escape.json", options).expect("traversal entry");
    writer.write_all(collection_json("Escape").as_bytes()).expect("write");
    writer.finish().expect("finish");
    let outcome = engine.import_zip(&zip_path, ImportOptions::default()).expect("import");
    assert_eq!(outcome.items.len(), 1, "{outcome:?}");
    assert_eq!(outcome.items[0].outcome.as_ref().expect("imported").name, "From zip");
    assert!(outcome.skipped.iter().any(|name| name.contains("traversal")), "{:?}", outcome.skipped);
    assert!(!fixture.dir.path().join("escape.json").exists(), "nothing is extracted outside the zip reader");
    assert_eq!(engine.collections().expect("collections").len(), 1);
}
