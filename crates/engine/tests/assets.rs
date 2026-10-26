//! Collection assets: storage, path validation, imports, rename/delete and duplication.

use postguy_engine::{Engine, EngineError};
use postguy_model::Id;
use postguy_store::StorePaths;

struct Rig {
    dir: tempfile::TempDir,
    engine: Engine,
    collection: Id,
}

impl Rig {
    fn new() -> Rig {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = Engine::open(StorePaths::new(dir.path().join("workspace"), dir.path().join("config"))).expect("engine");
        let collection = engine.create_collection("Assets").expect("collection").id;
        Rig { dir, engine, collection }
    }

    fn source(&self, relative: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = self.dir.path().join("src").join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(&path, bytes).expect("write source");
        path
    }
}

fn reason(err: EngineError) -> String {
    match err {
        EngineError::BadRequest { reason } => reason,
        other => other.to_string(),
    }
}

fn paths(rig: &Rig) -> Vec<String> {
    rig.engine.assets(&rig.collection).expect("list").into_iter().map(|info| info.path).collect()
}

#[test]
fn import_list_read_rename_delete() {
    let rig = Rig::new();
    assert!(paths(&rig).is_empty());
    let a = rig.source("a.jpg", b"AAAA");
    let folder = rig.source("imgs/b.png", b"BB");
    rig.source("imgs/deep/c.txt", b"C");
    let outcome = rig.engine.import_assets(&rig.collection, &[a, folder.parent().expect("folder").to_path_buf()], "", false).expect("import");
    assert!(outcome.skipped.is_empty(), "{:?}", outcome.skipped);
    assert_eq!(paths(&rig), ["a.jpg", "imgs/b.png", "imgs/deep/c.txt"]);
    assert_eq!(rig.engine.read_asset(&rig.collection, "imgs/b.png").expect("read"), b"BB");
    assert_eq!(rig.engine.list_asset_names(&rig.collection, "").expect("names"), ["a.jpg", "imgs"]);
    assert_eq!(rig.engine.list_asset_names(&rig.collection, "imgs").expect("names"), ["b.png", "deep"]);

    let moved = rig.engine.rename_asset(&rig.collection, "a.jpg", "pics/a2.jpg").expect("rename");
    assert_eq!((moved.path.as_str(), moved.size), ("pics/a2.jpg", 4));
    let err = rig.engine.rename_asset(&rig.collection, "pics/a2.jpg", "imgs/b.png").expect_err("exists");
    assert!(reason(err).contains("asset `imgs/b.png` already exists"));

    rig.engine.delete_asset(&rig.collection, "pics/a2.jpg").expect("delete");
    assert_eq!(paths(&rig), ["imgs/b.png", "imgs/deep/c.txt"], "the .bak of a deleted asset is not listed");
    let missing = rig.engine.read_asset(&rig.collection, "pics/a2.jpg").expect_err("gone");
    assert!(reason(missing).contains("does not exist in this collection; import it on the collection's Assets tab"));
}

#[test]
fn hostile_paths_are_refused() {
    let rig = Rig::new();
    for path in ["../x", "a/../../x", "/etc/passwd", ".hidden", "dir/.env", "a\\b", ""] {
        let err = rig.engine.read_asset(&rig.collection, path).expect_err(path);
        assert!(matches!(err, EngineError::BadRequest { .. }), "{path}: {err}");
    }
    assert!(reason(rig.engine.read_asset(&rig.collection, "../x").expect_err("dotdot")).contains("may not contain `..`"));
    assert!(rig.engine.delete_asset(&rig.collection, "../x").is_err());
    assert!(rig.engine.rename_asset(&rig.collection, "a", "../b").is_err());
    let bad_id = rig.engine.read_asset(&Id("../../etc".into()), "a").expect_err("bad id");
    assert!(matches!(bad_id, EngineError::BadRequest { .. }));
}

#[cfg(unix)]
#[test]
fn symlinks_cannot_escape_the_assets_directory() {
    let rig = Rig::new();
    let secret = rig.source("outside/secret.txt", b"top secret");
    let root = rig.engine.store_paths().assets_dir(&rig.collection.0).expect("assets dir");
    std::fs::create_dir_all(&root).expect("root");
    std::os::unix::fs::symlink(&secret, root.join("link.txt")).expect("file link");
    std::os::unix::fs::symlink(secret.parent().expect("dir"), root.join("linkdir")).expect("dir link");
    let err = rig.engine.read_asset(&rig.collection, "link.txt").expect_err("file symlink");
    assert!(reason(err).contains("outside the collection assets"));
    let err = rig.engine.read_asset(&rig.collection, "linkdir/secret.txt").expect_err("dir symlink");
    assert!(reason(err).contains("outside the collection assets"));
    assert!(paths(&rig).is_empty(), "symlinks are never listed");
    // An import cannot write through a symlinked directory either.
    let source = rig.source("x.txt", b"x");
    let outcome = rig.engine.import_assets(&rig.collection, &[source], "linkdir", false).expect("import");
    assert!(outcome.imported.is_empty() && outcome.skipped.iter().any(|line| line.contains("outside")), "{outcome:?}");
    assert!(!secret.parent().expect("dir").join("x.txt").exists());
}

#[test]
fn name_clashes_get_a_numbered_suffix_unless_replacing() {
    let rig = Rig::new();
    let first = rig.source("one/pic.jpg", b"1");
    let second = rig.source("two/pic.jpg", b"22");
    let third = rig.source("three/pic.jpg", b"333");
    rig.engine.import_assets(&rig.collection, &[first], "", false).expect("first");
    let outcome = rig.engine.import_assets(&rig.collection, &[second.clone(), third], "", false).expect("clash");
    let imported: Vec<&str> = outcome.imported.iter().map(|info| info.path.as_str()).collect();
    assert_eq!(imported, ["pic (2).jpg", "pic (3).jpg"]);
    let outcome = rig.engine.import_assets(&rig.collection, &[second], "", true).expect("replace");
    assert_eq!(outcome.imported.first().map(|info| info.path.as_str()), Some("pic.jpg"));
    assert_eq!(rig.engine.read_asset(&rig.collection, "pic.jpg").expect("replaced"), b"22");
}

#[test]
fn imports_skip_bad_sources_with_reasons_and_need_a_collection() {
    let rig = Rig::new();
    let hidden = rig.source(".env", b"SECRET=1");
    let ok = rig.source("fine.txt", b"ok");
    let outcome = rig.engine.import_assets(&rig.collection, &[hidden, ok, rig.dir.path().join("nope.txt")], "", false).expect("import");
    assert_eq!(outcome.imported.len(), 1);
    assert_eq!(outcome.skipped.len(), 2, "{:?}", outcome.skipped);
    let err = rig.engine.import_assets(&Id("ghost".into()), &[], "", false).expect_err("no collection");
    assert!(matches!(err, EngineError::NotFound { .. }), "{err}");
}

#[test]
fn duplicating_a_collection_copies_its_assets() {
    let rig = Rig::new();
    let source = rig.source("keep/me.txt", b"copied");
    rig.engine.import_assets(&rig.collection, &[source.parent().expect("dir").to_path_buf()], "", false).expect("import");
    let copy = rig.engine.duplicate_collection(&rig.collection, Some("Copy")).expect("duplicate");
    assert_ne!(copy.id, rig.collection);
    assert_eq!(rig.engine.read_asset(&copy.id, "keep/me.txt").expect("copied asset"), b"copied");
    assert_eq!(rig.engine.read_asset(&rig.collection, "keep/me.txt").expect("source intact"), b"copied");
}
