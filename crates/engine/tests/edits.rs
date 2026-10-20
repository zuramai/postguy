//! Engine workspace edits used by the sidebar: collection/environment CRUD, request/folder
//! creation, rename, duplicate (fresh ids), move/copy with cycle refusal, examples, trash
//! parking and history clearing.

use postguy_engine::Engine;
use postguy_model::{Item, RequestKind};
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
}

fn find<'a>(items: &'a [Item], id: &postguy_model::Id) -> Option<&'a Item> {
    for item in items {
        if item.id() == id {
            return Some(item);
        }
        if let Item::Folder(folder) = item
            && let Some(found) = find(&folder.items, id)
        {
            return Some(found);
        }
    }
    None
}

#[test]
fn collections_folders_and_requests_round_trip_and_rename() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let collection = engine.create_collection("Payments").expect("collection");
    let folder = engine.create_folder(&collection.id, None, "v1").expect("folder");
    let request = engine.create_request(&collection.id, Some(&folder.id), "http", "Charge").expect("request");
    assert_eq!(request.kind.protocol(), "http");
    engine.rename_item(&collection.id, &request.id, "Authorize").expect("rename");
    engine.rename_collection(&collection.id, "Billing").expect("rename collection");

    let loaded = &engine.collections().expect("collections")[0];
    assert_eq!(loaded.name, "Billing");
    match find(&loaded.items, &request.id) {
        Some(Item::Request(request)) => assert_eq!(request.name, "Authorize"),
        other => panic!("request vanished: {other:?}"),
    }
}

#[test]
fn duplicate_regenerates_ids_and_move_refuses_cycles() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let collection = engine.create_collection("Tree").expect("collection");
    let folder = engine.create_folder(&collection.id, None, "outer").expect("folder");
    let inner = engine.create_folder(&collection.id, Some(&folder.id), "inner").expect("inner");
    let request = engine.create_request(&collection.id, Some(&inner.id), "http", "Leaf").expect("request");

    engine.duplicate_item(&collection.id, &folder.id).expect("duplicate folder");
    let loaded = &engine.collections().expect("collections")[0];
    let folders: Vec<&Item> = loaded.items.iter().collect();
    assert_eq!(folders.len(), 2, "the duplicated folder is a sibling");
    let original_ids: Vec<&str> = loaded.items.iter().map(|item| item.id().0.as_str()).collect();
    assert_eq!(original_ids.iter().filter(|id| **id == folder.id.0).count(), 1);
    let copy_id = original_ids.iter().find(|id| **id != folder.id.0).expect("copy id");
    let copy = find(&loaded.items, &postguy_model::Id(copy_id.to_string())).expect("copy");
    match copy {
        Item::Folder(copy) => {
            assert_ne!(copy.id, folder.id);
            assert_ne!(copy.items.first().map(|item| item.id().clone()), Some(inner.id.clone()), "nested ids are fresh");
            let leaf = copy.items.first().and_then(|child| match child {
                Item::Folder(folder) => folder.items.first(),
                _ => None,
            });
            assert!(matches!(leaf, Some(Item::Request(leaf)) if leaf.id != request.id), "the leaf was copied with a fresh id");
        }
        _ => panic!("copy is not a folder"),
    }

    let error = engine.move_item(&collection.id, &folder.id, Some(&inner.id), 0, false).expect_err("cycle");
    assert!(error.to_string().contains("inside itself"), "{error}");
    // A normal move keeps the id and lands under the collection root.
    engine.move_item(&collection.id, &request.id, None, 0, false).expect("move");
    let loaded = &engine.collections().expect("collections")[0];
    assert!(matches!(loaded.items.first(), Some(Item::Request(item)) if item.id == request.id));
}

#[test]
fn examples_are_created_renamed_duplicated_and_deleted() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let collection = engine.create_collection("Examples").expect("collection");
    let request = engine.create_request(&collection.id, None, "http", "Echo").expect("request");
    let example = engine.create_example(&collection.id, &request.id, "200 OK").expect("example");
    engine.rename_item(&collection.id, &example.id, "Success").expect("rename example");
    engine.duplicate_item(&collection.id, &example.id).expect("duplicate example");
    let loaded = &engine.collections().expect("collections")[0];
    match find(&loaded.items, &request.id) {
        Some(Item::Request(request)) => {
            let names: Vec<&str> = request.examples.iter().map(|example| example.name.as_str()).collect();
            assert_eq!(names, vec!["Success", "Success Copy"]);
            let ids: Vec<&str> = request.examples.iter().map(|example| example.id.0.as_str()).collect();
            assert!(ids.contains(&example.id.0.as_str()));
        }
        _ => panic!("request vanished"),
    }
    engine.delete_item(&collection.id, &example.id).expect("delete example");
    let loaded = &engine.collections().expect("collections")[0];
    match find(&loaded.items, &request.id) {
        Some(Item::Request(request)) => assert_eq!(request.examples.len(), 1),
        _ => panic!("request vanished"),
    }
    let parked = std::fs::read_dir(engine.store_paths().trash_dir()).expect("trash dir").filter_map(Result::ok).count();
    assert!(parked > 0, "the delete parked a record");
}

#[test]
fn inherited_auth_walks_folders_and_the_collection() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let mut collection = engine.create_collection("Auth").expect("collection");
    collection.auth =
        Some(postguy_model::AuthConfig::Basic(Box::new(postguy_model::auth::BasicAuth { username: "root".into(), ..Default::default() })));
    engine.save_collection(&collection).expect("save");
    let folder = engine.create_folder(&collection.id, None, "inner").expect("folder");
    let mut folder_value = engine.folder(&collection.id, &folder.id).expect("folder read");
    folder_value.auth =
        Some(postguy_model::AuthConfig::Bearer(Box::new(postguy_model::auth::BearerAuth { token: "t".into(), ..Default::default() })));
    engine.save_folder(&collection.id, &folder_value).expect("save folder");
    let inside = engine.create_request(&collection.id, Some(&folder.id), "http", "Inside").expect("request");
    let outside = engine.create_request(&collection.id, None, "http", "Outside").expect("request");
    assert_eq!(engine.inherited_auth_kind(&collection.id, &inside.id).expect("inside"), Some("bearer".into()));
    assert_eq!(engine.inherited_auth_kind(&collection.id, &outside.id).expect("outside"), Some("basic".into()));
    folder_value.auth = Some(postguy_model::AuthConfig::NoAuth);
    engine.save_folder(&collection.id, &folder_value).expect("save folder");
    assert_eq!(engine.inherited_auth_kind(&collection.id, &inside.id).expect("inside"), Some("noauth".into()));
}

#[test]
fn deleting_an_item_parks_it_in_the_trash_then_saves_the_collection() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let collection = engine.create_collection("Trash").expect("collection");
    let request = engine.create_request(&collection.id, None, "http", "Old").expect("request");
    engine.delete_item(&collection.id, &request.id).expect("delete");
    let loaded = &engine.collections().expect("collections")[0];
    assert!(loaded.items.is_empty());
    let parked = std::fs::read_dir(engine.store_paths().trash_dir()).expect("trash dir").filter_map(Result::ok).count();
    assert!(parked > 0, "a trash record exists");
}

#[test]
fn pasted_items_get_copy_suffixes_on_a_name_clash() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let collection = engine.create_collection("Paste").expect("collection");
    let request = engine.create_request(&collection.id, None, "http", "Charge").expect("request");
    let source = engine.collections().expect("collections")[0].items.first().cloned().expect("item");
    engine.paste_item(&collection.id, None, source.clone(), true).expect("paste once");
    engine.paste_item(&collection.id, None, source, true).expect("paste twice");
    let loaded = &engine.collections().expect("collections")[0];
    let names: Vec<&str> = loaded.items.iter().map(Item::name).collect();
    assert_eq!(names, vec!["Charge", "Charge Copy", "Charge Copy 2"]);
    assert!(loaded.items.iter().filter(|item| item.id() == &request.id).count() == 1, "the original id is untouched");
}

#[test]
fn environments_create_activate_duplicate_and_delete() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let environment = engine.create_environment("Staging").expect("environment");
    engine.set_active_environment(Some(environment.id.clone())).expect("activate");
    assert_eq!(engine.active_environment().expect("active"), Some(environment.id.clone()));
    let copy = engine.duplicate_environment(&environment.id, Some("Production")).expect("copy");
    assert_ne!(copy.id, environment.id);
    assert_eq!(engine.environments().expect("environments").len(), 2);
    engine.delete_environment(&environment.id).expect("delete");
    assert_ne!(engine.active_environment().expect("active"), Some(environment.id));
    assert_eq!(engine.environments().expect("environments").len(), 1);
}

#[test]
fn history_clear_is_atomic_and_edits_stay_out_of_it() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    engine.clear_history().expect("clear empty");
    let collection = engine.create_collection("No send").expect("collection");
    engine.rename_collection(&collection.id, "Still no send").expect("rename");
    assert!(engine.history(usize::MAX).expect("history").is_empty(), "edits never write history");
}

#[test]
fn request_protocols_are_created_with_their_kind() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let collection = engine.create_collection("Kinds").expect("collection");
    let grpc = engine.create_request(&collection.id, None, "grpc", "RPC").expect("grpc");
    let ws = engine.create_request(&collection.id, None, "websocket", "Socket").expect("ws");
    assert!(matches!(grpc.kind, RequestKind::Grpc(_)));
    assert!(matches!(ws.kind, RequestKind::WebSocket(_)));
    let error = engine.create_request(&collection.id, None, "soap", "Nope").expect_err("unknown protocol");
    assert!(error.to_string().contains("unknown protocol"), "{error}");
}
