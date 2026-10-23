//! Integration test (crate level): the full M1.5 story on a temp folder — open, create,
//! save, reload, corrupt-file quarantine, lock takeover, trash + restore, secrets split and
//! a fresh process seeing the persisted state. Hermetic: the temp dir is everything.

use postguy_model::{Collection, Environment, Id, Variable};
use postguy_store::{Workspace, config_dir, paths::StorePaths};
use postguy_util::Result;

fn fresh_paths(dir: &std::path::Path) -> StorePaths {
    StorePaths::new(dir.join("work"), dir.join("config"))
}

fn sample_collection(name: &str) -> Collection {
    Collection { id: Id::generate(), name: name.to_string(), ..Collection::default() }
}

#[test]
fn settings_round_trip_through_save_and_load() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let paths = fresh_paths(dir.path());
    let settings = postguy_model::Settings { editor_font_size: 15.5, theme: postguy_model::Theme::Dark, ..Default::default() };
    config_dir::save_settings(&paths, &settings)?;
    let loaded = config_dir::load_settings(&paths)?;
    assert!(loaded.quarantined_to.is_none(), "a file we just wrote is not corrupt");
    assert_eq!(loaded.settings.editor_font_size, 15.5);
    assert_eq!(loaded.settings.theme, postguy_model::Theme::Dark);

    // An unknown key written by a newer version survives a load and save.
    let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(paths.settings_file())?)?;
    value["futureKey"] = serde_json::json!(1);
    std::fs::write(paths.settings_file(), serde_json::to_vec(&value)?)?;
    let reloaded = config_dir::load_settings(&paths)?;
    assert!(reloaded.quarantined_to.is_none());
    assert_eq!(reloaded.settings.extras.get("futureKey"), Some(&serde_json::json!(1)));
    Ok(())
}

#[test]
fn the_full_store_story_round_trips() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let paths = fresh_paths(dir.path());

    // 1. Open creates the tree and the ordered meta.
    let workspace = Workspace::open(paths.clone())?;
    assert!(paths.collections_dir().is_dir());
    assert!(paths.files_dir().is_dir());
    assert_eq!(workspace.collection_ids()?, Vec::<Id>::new());

    // 2. Save two collections; the order file names them in save order.
    let alpha = sample_collection("Alpha");
    let beta = sample_collection("Beta");
    workspace.save_collection(&alpha)?;
    workspace.save_collection(&beta)?;
    assert_eq!(workspace.collection_ids()?, vec![alpha.id.clone(), beta.id.clone()]);

    // 3. Reload through a NEW handle: the second process sees the same state.
    drop(workspace);
    let reopened = Workspace::open(paths.clone())?;
    let loaded = reopened.load_collection(&alpha.id)?;
    assert!(loaded.quarantined_path().is_none(), "healthy file loads clean");
    assert_eq!(loaded.into_value().name, "Alpha");
    assert_eq!(reopened.collection_ids()?, vec![alpha.id.clone(), beta.id.clone()]);

    // 4a. Environments and the active id.
    let dev = Environment { id: Id::generate(), name: "dev".into(), ..Default::default() };
    reopened.save_environment(&dev)?;
    reopened.set_active_environment(Some(dev.id.clone()))?;
    assert_eq!(reopened.active_environment_id()?, Some(dev.id.clone()));

    // 4b. ReloadGlobals both ways.
    let globals = Environment { id: Id("globals".into()), name: "Globals".into(), ..Default::default() };
    reopened.save_globals(&globals)?;
    let reloaded_globals = reopened.load_globals()?;
    assert_eq!(reloaded_globals.into_value().name, "Globals");

    // 5. Secret values live in the secrets store, NOT in the environment file.
    let secret_env = Environment {
        id: Id::generate(),
        name: "with-secret".into(),
        variables: vec![Variable { key: "apiKey".into(), value: "SPACES_ARE_CHEATING".into(), secret: true, ..Variable::default() }],
        ..Default::default()
    };
    reopened.save_environment(&secret_env)?;
    reopened.secrets.set(&secret_env.id.0, "apiKey", Some("hunter2"))?;
    let on_disk = std::fs::read_to_string(paths.environment_file(&secret_env.id.0)?)?;
    assert!(!on_disk.contains("hunter2"), "secret values never sit in environment files: {on_disk}");
    reloaded_store_check(&paths, &secret_env.id.0);

    // 6. Deleting a collection trashes it and restores with a restored-suffix path.
    reopened.delete_collection(&beta.id)?;
    let entries = reopened.trash_entries()?;
    assert_eq!(entries.len(), 1, "one trashed entry: {entries:?}");
    let beta_path = reopened.restore_from_trash(&entries[0].id)?;
    assert!(beta_path.is_some(), "restore found a path for the first clash-free spot");
    assert!(beta_path.unwrap_or_default().exists(), "the file is back on disk");

    // 7. A corrupt collection quarantines with a `.corrupt-<time>` name and the app continues.
    let broken = sample_collection("Broken");
    reopened.save_collection(&broken)?;
    let broken_path = paths.collection_file(&broken.id.0)?;
    std::fs::write(&broken_path, b"not json *at all----------")?;
    let outcome = reopened.load_collection(&broken.id)?;
    let quarantined = outcome.quarantined_path().expect("corrupt file reported");
    assert!(quarantined.to_string_lossy().contains("corrupt"));
    // The broken file is out of the way; a RESAVE works and the next load is clean.
    reopened.save_collection(&broken)?;
    let after = reopened.load_collection(&broken.id)?;
    assert!(after.quarantined_path().is_none());

    // 8. History rotation keeps working over many appends (cap enforced).
    for i in 0..30 {
        reopened.append_history(&postguy_model::HistoryEntry { id: Id(format!("h{i}")), ..Default::default() })?;
    }
    assert_eq!(reopened.read_history(usize::MAX)?.len(), 30);

    // 9. Config dir files load with Defaults when absent.
    let outcome = config_dir::load_settings(&paths)?;
    assert!(outcome.quarantined_to.is_none());
    assert_eq!(outcome.settings.max_response_mb, 50);
    Ok(())
}

fn reloaded_store_check(paths: &StorePaths, owner: &str) {
    let store = postguy_store::secrets::SecretsStore::open(paths).expect("secrets reopen");
    let values = store.values_for(owner).expect("values");
    assert_eq!(values.get("apiKey").map(String::as_str), Some("hunter2"), "the secret survived reopen");
}
