//! Integration: proxy routing over real sockets — absolute-form through an HTTP proxy, proxy
//! basic auth, a CONNECT tunnel for HTTPS, SOCKS5 vs SOCKS5h address forms, bypass lists and
//! the HTTP/2 refusal for plain proxied HTTP.

use postguy_http::{Cancel, Client, ClientConfig, HttpVersionPreference, RequestSpec, TlsMode};
use postguy_model::{Extras, ProxyMode, ProxyProtocol, ProxySettings};
use postguy_testkit::{ProxyServer, Server, Socks5Server};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime")
}

fn spec(url: String) -> RequestSpec {
    RequestSpec { max_body_bytes: 1 << 20, ..RequestSpec::get(url) }
}

fn custom_proxy(protocol: ProxyProtocol, host: &str, port: u16) -> ProxySettings {
    ProxySettings {
        mode: ProxyMode::Custom,
        protocol,
        host: host.to_string(),
        port,
        username: String::new(),
        password: String::new(),
        apply_to_http: true,
        apply_to_https: true,
        bypass: String::new(),
        extras: Extras::default(),
    }
}

fn client_with(proxy: ProxySettings) -> Client {
    Client::new(ClientConfig { proxy, ..ClientConfig::default() }).expect("client")
}

fn echo_request(body: &[u8]) -> (String, String) {
    let parsed: serde_json::Value = serde_json::from_slice(body).expect("echo json");
    (parsed["path"].as_str().unwrap_or_default().to_string(), parsed["query"].as_str().unwrap_or_default().to_string())
}

#[test]
fn plain_http_travels_through_an_http_proxy_in_absolute_form() {
    let origin = Server::start();
    let proxy = ProxyServer::start();
    let target = format!("{}/echo?x=1", origin.base_url());
    let client = client_with(custom_proxy(ProxyProtocol::Http, "127.0.0.1", proxy.addr().port()));
    let runtime = runtime();
    let response = runtime.block_on(client.send(spec(target.clone()), &Cancel::new())).expect("send");
    assert_eq!(response.status.as_u16(), 200);
    assert_eq!(echo_request(&response.body), ("/echo".to_string(), "x=1".to_string()), "the origin received the request through the proxy");
    let seen = proxy.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].target, target, "absolute-form request target");
}

#[test]
fn proxy_basic_auth_is_attached_only_for_plain_proxied_requests() {
    let origin = Server::start();
    let proxy = ProxyServer::start_with_auth("proxy-user", "proxy-pass");
    let runtime = runtime();

    // Without credentials the proxy answers 407 (not a transport error: the user sees it).
    let bare = client_with(custom_proxy(ProxyProtocol::Http, "127.0.0.1", proxy.addr().port()));
    let refused = runtime.block_on(bare.send(spec(format!("{}/echo", origin.base_url())), &Cancel::new())).expect("send");
    assert_eq!(refused.status.as_u16(), 407);

    // With credentials the request goes through.
    let mut settings = custom_proxy(ProxyProtocol::Http, "127.0.0.1", proxy.addr().port());
    settings.username = "proxy-user".into();
    settings.password = "proxy-pass".into();
    let authed = client_with(settings);
    let response = runtime.block_on(authed.send(spec(format!("{}/echo", origin.base_url())), &Cancel::new())).expect("send");
    assert_eq!(response.status.as_u16(), 200);
    let last = proxy.seen().pop().expect("proxy saw it");
    assert_eq!(last.proxy_authorization.as_deref(), Some("Basic cHJveHktdXNlcjpwcm94eS1wYXNz"));
}

#[test]
fn https_runs_inside_a_connect_tunnel() {
    let origin = Server::start_tls();
    let proxy = ProxyServer::start();
    let mut settings = custom_proxy(ProxyProtocol::Http, "127.0.0.1", proxy.addr().port());
    settings.apply_to_http = false;
    let client = Client::new(ClientConfig { proxy: settings, tls: TlsMode::Insecure, ..ClientConfig::default() }).expect("client");
    let runtime = runtime();
    let response = runtime.block_on(client.send(spec(format!("{}/echo", origin.base_url())), &Cancel::new())).expect("send");
    assert_eq!(response.status.as_u16(), 200);
    assert_eq!(proxy.connect_count(), 1, "one CONNECT tunnel");
    let seen = proxy.seen();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].target.starts_with("127.0.0.1:"), "CONNECT authority-form: {:?}", seen[0].target);
}

#[test]
fn socks5h_sends_the_host_name_and_socks5_resolves_locally() {
    let origin = Server::start();
    let socks = Socks5Server::start();
    let runtime = runtime();
    let url = format!("{}/echo", origin.base_url());

    let remote = client_with(custom_proxy(ProxyProtocol::Socks5h, "127.0.0.1", socks.addr().port()));
    let response = runtime.block_on(remote.send(spec(url.clone()), &Cancel::new())).expect("socks5h send");
    assert_eq!(response.status.as_u16(), 200);
    let seen = socks.seen();
    assert_eq!(seen.last().map(|seen| seen.address_type), Some("domain"), "socks5h sends the host name");
    assert_eq!(seen.last().map(|seen| seen.port), Some(origin.addr().port()));

    let local = client_with(custom_proxy(ProxyProtocol::Socks5, "127.0.0.1", socks.addr().port()));
    let response = runtime.block_on(local.send(spec(url), &Cancel::new())).expect("socks5 send");
    assert_eq!(response.status.as_u16(), 200);
    assert_eq!(socks.seen().last().map(|seen| seen.address_type), Some("ipv4"), "socks5 resolves locally");
}

#[test]
fn a_bypass_goes_direct_even_when_the_proxy_address_is_dead() {
    let origin = Server::start();
    let mut settings = custom_proxy(ProxyProtocol::Http, "127.0.0.1", 9);
    settings.bypass = "127.0.0.1, *.internal".into();
    let response =
        runtime().block_on(client_with(settings).send(spec(format!("{}/echo", origin.base_url())), &Cancel::new())).expect("bypassed send");
    assert_eq!(response.status.as_u16(), 200);
}

#[test]
fn http2_over_a_plain_http_proxy_is_refused_with_an_actionable_error() {
    let origin = Server::start();
    let proxy = ProxyServer::start();
    let client = Client::new(ClientConfig {
        proxy: custom_proxy(ProxyProtocol::Http, "127.0.0.1", proxy.addr().port()),
        version: HttpVersionPreference::Http2,
        ..ClientConfig::default()
    })
    .expect("client");
    let error = runtime().block_on(client.send(spec(format!("{}/echo", origin.base_url())), &Cancel::new())).expect_err("must refuse");
    assert!(error.to_string().contains("HTTP/2"), "{error}");
    assert!(proxy.seen().is_empty(), "nothing reached the proxy");
}
