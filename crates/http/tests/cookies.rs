//! Integration: the cookie jar over real sockets — capture on a redirect hop, send on the next
//! hop, merge with a manual Cookie header, respect the per-request disable, and persist across
//! "sessions" through the JSON shape the store writes.

use std::sync::Mutex;

use postguy_http::{Cancel, Client, ClientConfig, CookieJar, RedirectPolicy, RequestSpec};
use postguy_testkit::Server;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime")
}

fn spec(url: String) -> RequestSpec {
    RequestSpec { max_body_bytes: 1 << 20, ..RequestSpec::get(url) }
}

fn echo_cookie_header(body: &[u8]) -> String {
    let parsed: serde_json::Value = serde_json::from_slice(body).expect("echo json");
    parsed["cookieHeader"].as_str().unwrap_or_default().to_string()
}

#[test]
fn set_cookie_on_a_redirect_hop_rides_the_next_hop() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let jar = Mutex::new(CookieJar::new());
    let start = format!("{}/cookies/redirect-set?name=hop&value=1&to=/cookies/echo", server.base_url());
    let outcome = runtime.block_on(client.send_with_cookies(spec(start), RedirectPolicy::default(), &jar, false, &Cancel::new())).expect("send");
    assert_eq!(outcome.hops.len(), 1, "one redirect hop");
    assert_eq!(echo_cookie_header(&outcome.response.body), "hop=1", "the hop's cookie was captured and sent");
    assert_eq!(jar.lock().expect("jar").cookies.len(), 1);
}

#[test]
fn jar_cookies_merge_with_a_manual_cookie_header() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let jar = Mutex::new(CookieJar::new());
    jar.lock().expect("jar").store_from_headers(&format!("{}/", server.base_url()), ["jar=1; Path=/"]);
    let request = RequestSpec { headers: vec![("Cookie".into(), "manual=2".into())], ..spec(format!("{}/cookies/echo", server.base_url())) };
    let outcome = runtime.block_on(client.send_with_cookies(request, RedirectPolicy::default(), &jar, false, &Cancel::new())).expect("send");
    assert_eq!(echo_cookie_header(&outcome.response.body), "jar=1; manual=2");
}

#[test]
fn the_per_request_disable_sends_and_stores_nothing() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let jar = Mutex::new(CookieJar::new());
    jar.lock().expect("jar").store_from_headers(&format!("{}/", server.base_url()), ["jar=1; Path=/"]);
    // Disabled: the stored cookie is not sent, and a Set-Cookie on a hop is not stored.
    let start = format!("{}/cookies/redirect-set?name=ignored&value=1&to=/cookies/echo", server.base_url());
    let outcome = runtime.block_on(client.send_with_cookies(spec(start), RedirectPolicy::default(), &jar, true, &Cancel::new())).expect("send");
    assert_eq!(echo_cookie_header(&outcome.response.body), "", "nothing was sent");
    assert_eq!(jar.lock().expect("jar").cookies.len(), 1, "nothing new stored");
}

#[test]
fn persistence_round_trips_through_the_store_json_shape() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let jar = Mutex::new(CookieJar::new());
    let _ = runtime
        .block_on(client.send_with_cookies(
            spec(format!("{}/cookies/redirect-set?name=persist&value=yes&to=/cookies/echo", server.base_url())),
            RedirectPolicy::default(),
            &jar,
            false,
            &Cancel::new(),
        ))
        .expect("seed the jar");

    // What the store would write and read back in a later session.
    let json = jar.lock().expect("jar").to_json();
    let session_two = Mutex::new(CookieJar::from_json(&json));
    let outcome = runtime
        .block_on(client.send_with_cookies(
            spec(format!("{}/cookies/echo", server.base_url())),
            RedirectPolicy::default(),
            &session_two,
            false,
            &Cancel::new(),
        ))
        .expect("send from persisted jar");
    assert_eq!(echo_cookie_header(&outcome.response.body), "persist=yes");
}

#[test]
fn expired_and_secure_rules_hold_over_the_wire() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = || SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let jar = Mutex::new(CookieJar::new());
    {
        let mut jar = jar.lock().expect("jar");
        // Secure cookie stored from https would not ride this http request.
        jar.store_from_headers(&format!("{}/", server.base_url()).replace("http://", "https://"), ["secure=1; Secure; Path=/"]);
        // An already-expired cookie is not sent.
        jar.store_from_headers_at(&format!("{}/", server.base_url()), ["old=1; Max-Age=1; Path=/"], now() - 60_000);
    }
    let outcome = runtime
        .block_on(client.send_with_cookies(
            spec(format!("{}/cookies/echo", server.base_url())),
            RedirectPolicy::default(),
            &jar,
            false,
            &Cancel::new(),
        ))
        .expect("send");
    assert_eq!(echo_cookie_header(&outcome.response.body), "", "secure over http and expired cookies stay home");
}
