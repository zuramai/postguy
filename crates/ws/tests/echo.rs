//! WebSocket handshake, echo, binary, greeting and close against the testkit server.

use postguy_ws::{WsConnection, WsEvent, WsOptions};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime")
}

#[test]
fn text_and_binary_round_trip() {
    let server = postguy_testkit::WsEchoServer::start().expect("ws server");
    runtime().block_on(async {
        let mut connection = WsConnection::connect(&WsOptions { url: server.url(), ..WsOptions::default() }).await.expect("connect");
        connection.send_text("hello").await.expect("send");
        assert_eq!(connection.next().await, Some(WsEvent::Text("hello".into())));
        connection.send_binary(vec![1, 2, 3]).await.expect("send binary");
        assert_eq!(connection.next().await, Some(WsEvent::Binary(vec![1, 2, 3])));
        connection.close(1000, "done").await.expect("close");
    });
}

#[test]
fn greeting_and_custom_headers_reach_the_handshake() {
    let server = postguy_testkit::WsEchoServer::start_with_greeting(Some("welcome".into())).expect("ws server");
    runtime().block_on(async {
        let options =
            WsOptions { url: server.url(), headers: vec![("X-Trace".into(), "abc".into())], protocols: vec!["chat".into()], ..WsOptions::default() };
        let mut connection = WsConnection::connect(&options).await.expect("connect");
        assert_eq!(connection.next().await, Some(WsEvent::Text("welcome".into())));
        connection.close(1000, "bye").await.expect("close");
    });
}

#[test]
fn invalid_urls_and_schemes_are_actionable() {
    let runtime = runtime();
    runtime.block_on(async {
        let error = WsConnection::connect(&WsOptions { url: "http://example.test".into(), ..WsOptions::default() }).await.expect_err("scheme");
        assert!(error.contains("ws://"));
        let error = WsConnection::connect(&WsOptions::default()).await.expect_err("empty");
        assert!(error.contains("empty"));
    });
}
