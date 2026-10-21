//! Editing a WebSocket draft must not persist it just because the user connects.

use postguy_engine::Engine;
use postguy_model::RequestKind;
use postguy_store::StorePaths;

#[test]
fn connecting_an_edited_draft_leaves_the_stored_request_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = Engine::open(StorePaths::new(dir.path().join("ws"), dir.path().join("cfg"))).expect("engine");
    let server = postguy_testkit::WsEchoServer::start().expect("ws server");
    let collection = engine.create_collection("WS").expect("collection");
    let request = engine.create_request(&collection.id, None, "websocket", "Socket").expect("request");
    let mut stored = engine.request(&collection.id, &request.id).expect("stored");
    let RequestKind::WebSocket(mut websocket) = stored.kind.clone() else { panic!("websocket kind") };
    websocket.url = "ws://127.0.0.1:9/never".into();
    stored.kind = RequestKind::WebSocket(websocket.clone());
    engine.save_request(&collection.id, &stored).expect("save");
    let mut draft = websocket;
    draft.url = server.url().to_string();
    let session = engine.ws_connect_draft(&collection.id, &request.id, &draft).expect("draft connect");
    for _ in 0..200 {
        if engine.ws_connected(&session).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(engine.ws_connected(&session).expect("connected"), "draft connection opened");
    let after = engine.request(&collection.id, &request.id).expect("after");
    let RequestKind::WebSocket(after_ws) = after.kind else { panic!("kind") };
    assert_eq!(after_ws.url, "ws://127.0.0.1:9/never", "connect must not save the draft");
    let _ = engine.ws_disconnect(&session, 1000, "test".into());
}
