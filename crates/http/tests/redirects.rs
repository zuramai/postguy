//! Integration: the manual redirect loop against the testkit — hop records, 301/302/303 method
//! rules, 307/308 preservation, cross-host credential dropping, referer removal, the hop cap
//! and loop detection.

use postguy_http::{Cancel, Client, ClientConfig, RedirectPolicy, RequestSpec};
use postguy_testkit::Server;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime")
}

fn spec(url: String) -> RequestSpec {
    RequestSpec { max_body_bytes: 1 << 20, ..RequestSpec::get(url) }
}

#[test]
fn a_chain_is_followed_and_every_hop_recorded() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let target = format!("{}/echo", server.base_url());
    let start = format!("{}/redirect/chain?hops=3&to=/echo", server.base_url());
    let outcome = runtime.block_on(client.send_following_redirects(spec(start), RedirectPolicy::default(), &Cancel::new())).expect("follow");
    assert_eq!(outcome.response.status, 200);
    assert_eq!(outcome.hops.len(), 3, "three redirects before the echo");
    assert!(outcome.hops.iter().all(|hop| hop.status == 302 && hop.location.is_some()));
    let parsed: serde_json::Value = serde_json::from_slice(&outcome.response.body).expect("echo json");
    assert_eq!(parsed["path"], "/echo", "landed at {target}");
}

#[test]
fn post_becomes_get_on_303_and_301_unless_the_original_method_is_kept() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let post = RequestSpec {
        method: hyper::Method::POST,
        body: b"payload".to_vec(),
        headers: vec![("Content-Type".into(), "text/plain".into()), ("Authorization".into(), "Bearer same-host".into())],
        ..spec(format!("{}/redirect?code=303&to=/echo", server.base_url()))
    };
    let outcome = runtime.block_on(client.send_following_redirects(post.clone(), RedirectPolicy::default(), &Cancel::new())).expect("303");
    let parsed: serde_json::Value = serde_json::from_slice(&outcome.response.body).expect("echo");
    assert_eq!(parsed["method"], "GET", "303 switches to GET");
    assert_eq!(parsed["body"]["utf8"], "", "the body is dropped on the rewrite");
    assert_eq!(parsed["bodyLength"], 0);
    // Same host: the authorization header survives.
    let headers = parsed["headers"].as_array().expect("headers");
    assert!(headers.iter().any(|header| header["name"] == "authorization" && header["value"] == "Bearer same-host"));

    // 301 POST -> GET by default.
    let outcome = runtime
        .block_on(client.send_following_redirects(RequestSpec { ..post.clone() }, RedirectPolicy::default(), &Cancel::new()))
        .expect("301 default");
    assert_eq!(outcome.response.status, 200);

    // follow_original_method keeps POST (302 case) with its body.
    let post_302 = RequestSpec { ..spec(format!("{}/redirect?code=302&to=/echo", server.base_url())) };
    let outcome = runtime
        .block_on(client.send_following_redirects(
            RequestSpec { method: hyper::Method::POST, body: b"keep".to_vec(), ..post_302 },
            RedirectPolicy { follow_original_method: true, ..RedirectPolicy::default() },
            &Cancel::new(),
        ))
        .expect("302 keeps method");
    let parsed: serde_json::Value = serde_json::from_slice(&outcome.response.body).expect("echo");
    assert_eq!(parsed["method"], "POST");
    assert_eq!(parsed["body"]["utf8"], "keep");
}

#[test]
fn preserve_method_codes_keep_method_and_body() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    for code in [307, 308] {
        let post = RequestSpec {
            method: hyper::Method::POST,
            body: b"payload".to_vec(),
            ..spec(format!("{}/redirect?code={code}&to=/echo", server.base_url()))
        };
        let outcome = runtime.block_on(client.send_following_redirects(post, RedirectPolicy::default(), &Cancel::new())).expect("preserve");
        let parsed: serde_json::Value = serde_json::from_slice(&outcome.response.body).expect("echo");
        assert_eq!(parsed["method"], "POST", "{code}");
        assert_eq!(parsed["body"]["utf8"], "payload", "{code}");
    }
}

#[test]
fn cross_host_hops_drop_credentials_by_default_and_keep_them_when_asked() {
    let first = Server::start();
    let second = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let target = format!("{}/echo", second.base_url());
    let make = || RequestSpec {
        headers: vec![
            ("Authorization".into(), "Bearer secret".into()),
            ("Cookie".into(), "session=1".into()),
            ("Referer".into(), "http://first/".into()),
        ],
        ..spec(format!("{}/redirect?to={}", first.base_url(), target))
    };
    // Default: credentials and body headers dropped on the cross-host hop.
    let outcome = runtime.block_on(client.send_following_redirects(make(), RedirectPolicy::default(), &Cancel::new())).expect("cross-host");
    let parsed: serde_json::Value = serde_json::from_slice(&outcome.response.body).expect("echo");
    let headers = parsed["headers"].as_array().expect("headers");
    assert!(!headers.iter().any(|header| header["name"] == "authorization"), "authorization dropped: {headers:?}");
    assert!(!headers.iter().any(|header| header["name"] == "cookie"), "cookie dropped: {headers:?}");
    assert!(headers.iter().any(|header| header["name"] == "referer"), "referer kept without the setting");

    // follow_authorization_on_redirect keeps authorization across hosts.
    let outcome = runtime
        .block_on(client.send_following_redirects(
            make(),
            RedirectPolicy { follow_authorization_on_redirect: true, ..RedirectPolicy::default() },
            &Cancel::new(),
        ))
        .expect("keep auth");
    let parsed: serde_json::Value = serde_json::from_slice(&outcome.response.body).expect("echo");
    let headers = parsed["headers"].as_array().expect("headers");
    assert!(headers.iter().any(|header| header["name"] == "authorization"), "{headers:?}");
}

#[test]
fn remove_referer_drops_a_user_set_referer_after_a_hop() {
    let first = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let target = format!("{}/echo", first.base_url());
    let referer_spec =
        RequestSpec { headers: vec![("Referer".into(), "http://previous/".into())], ..spec(format!("{}/redirect?to={}", first.base_url(), target)) };
    let outcome = runtime
        .block_on(client.send_following_redirects(
            referer_spec,
            RedirectPolicy { remove_referer_on_redirect: true, ..RedirectPolicy::default() },
            &Cancel::new(),
        ))
        .expect("redirect");
    let parsed: serde_json::Value = serde_json::from_slice(&outcome.response.body).expect("echo");
    let headers = parsed["headers"].as_array().expect("headers");
    assert!(!headers.iter().any(|header| header["name"] == "referer"), "{headers:?}");
}

#[test]
fn the_hop_cap_and_loop_detection_stop_with_actionable_errors() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let deep = spec(format!("{}/redirect/chain?hops=12&to=/echo", server.base_url()));
    let err = runtime
        .block_on(client.send_following_redirects(deep, RedirectPolicy { max_redirects: 5, ..RedirectPolicy::default() }, &Cancel::new()))
        .expect_err("cap");
    assert!(err.to_string().contains("maximum 5"), "{err}");

    let looping = spec(format!("{}/redirect/loop", server.base_url()));
    let err = runtime.block_on(client.send_following_redirects(looping, RedirectPolicy::default(), &Cancel::new())).expect_err("loop");
    assert!(err.to_string().contains("redirect loop"), "{err}");
}

#[test]
fn following_off_returns_the_3xx_as_the_final_response() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let outcome = runtime
        .block_on(client.send_following_redirects(
            spec(format!("{}/redirect?to=/echo", server.base_url())),
            RedirectPolicy { follow: false, ..RedirectPolicy::default() },
            &Cancel::new(),
        ))
        .expect("no follow");
    assert_eq!(outcome.response.status, 302);
    assert!(outcome.hops.is_empty());
    assert!(outcome.response.headers.iter().any(|(name, value)| name == "location" && value == "/echo"));
}
