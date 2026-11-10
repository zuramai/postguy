//! The update check and the verified asset download against the testkit's GitHub-shaped release
//! fixture (loopback only).

use postguy_engine::update::current_version;
use postguy_engine::{Cancel, Engine};
use postguy_store::StorePaths;
use postguy_testkit::Server;

fn engine_in(dir: &std::path::Path) -> Engine {
    let workspace = dir.join("work");
    let config = workspace.join(".postguy-config");
    std::fs::create_dir_all(&config).expect("config dir");
    Engine::open(StorePaths::new(&workspace, &config)).expect("engine")
}

#[test]
fn a_newer_release_is_offered_and_an_older_one_is_not() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = engine_in(dir.path());

    let check = engine.check_for_update_at(&format!("{}/releases/latest", server.base_url())).expect("check");
    assert!(check.available, "{check:?}");
    assert_eq!(check.current, current_version());
    assert_eq!(check.latest.version, "99.0.0");
    assert!(check.latest.asset("SHA256SUMS.txt").is_some());

    let check = engine.check_for_update_at(&format!("{}/releases/latest?version=0.0.1", server.base_url())).expect("check");
    assert!(!check.available, "an older release is not an update: {check:?}");
}

#[test]
fn a_download_follows_the_redirect_and_matches_its_checksum() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = engine_in(dir.path());
    let check = engine.check_for_update_at(&format!("{}/releases/latest", server.base_url())).expect("check");
    let downloads = dir.path().join("downloads");

    let asset = engine.download_update_asset(&check.latest, "postguy-test.bin", &downloads, &Cancel::new()).expect("download");
    assert_eq!(std::fs::read(&asset.path).expect("downloaded file"), postguy_testkit::routes::RELEASE_PAYLOAD);
    assert_eq!(asset.bytes, postguy_testkit::routes::RELEASE_PAYLOAD.len() as u64);
}

#[test]
fn a_checksum_mismatch_leaves_nothing_behind() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = engine_in(dir.path());
    let check = engine.check_for_update_at(&format!("{}/releases/latest", server.base_url())).expect("check");
    let downloads = dir.path().join("downloads");

    let error = engine.download_update_asset(&check.latest, "postguy-tampered.bin", &downloads, &Cancel::new()).expect_err("tampered");
    assert!(error.to_string().contains("failed its checksum"), "{error}");
    assert_eq!(std::fs::read_dir(&downloads).expect("downloads dir").count(), 0, "neither the file nor its .part survives");

    let error = engine.download_update_asset(&check.latest, "../escape.bin", &downloads, &Cancel::new()).expect_err("unsafe name");
    assert!(error.to_string().contains("not a release asset file name"), "{error}");
}
