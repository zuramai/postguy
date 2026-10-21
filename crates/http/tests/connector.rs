//! Integration: the connector and client against the testkit over real sockets — phase
//! timings present, TLS verified with the testkit CA, verification failures refused, the
//! insecure toggle working, timeouts and cancellation aborting, and the body cap enforced.

use std::time::Duration;

use postguy_http::{Cancel, Client, ClientConfig, HttpError, HttpVersionPreference, RequestSpec, TlsMode};
use postguy_testkit::Server;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime")
}

fn write_ca(dir: &std::path::Path, pem: &str) -> std::path::PathBuf {
    let path = dir.join("testkit-ca.pem");
    std::fs::write(&path, pem).expect("write CA");
    path
}

#[test]
fn a_plain_get_reports_status_body_and_phase_timings() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let spec = RequestSpec { headers: vec![("X-Probe".into(), "alpha".into())], ..RequestSpec::get(format!("{}/echo", server.base_url())) };
    let response = runtime.block_on(client.send(spec, &Cancel::new())).expect("send");
    assert_eq!(response.status, 200);
    assert_eq!(response.version, hyper::Version::HTTP_11, "plain testkit serves http/1.1");
    let parsed: serde_json::Value = serde_json::from_slice(&response.body).expect("json echo");
    assert_eq!(parsed["method"], "GET");
    assert_eq!(parsed["headers"][0]["name"], "x-probe", "sent header arrived");
    // Phase timings exist and are ordered realistically (a loopback round trip is fast).
    let timings = response.timings;
    assert!(timings.connect.total_ms < 5_000);
    assert!(timings.ttfb_ms < 5_000);
    assert!(timings.total_ms >= timings.ttfb_ms);
    assert!(client.last_connect_timings().is_some(), "connector recorded its phases");
}

#[test]
fn a_tls_request_verifies_against_the_user_ca() {
    let server = Server::start_tls();
    let dir = tempfile::tempdir().expect("tempdir");
    let ca = write_ca(dir.path(), server.ca_pem().expect("pem"));
    let runtime = runtime();

    // With the CA trusted: works.
    let client =
        Client::new(ClientConfig { tls: TlsMode::Verify { ca_pem_path: Some(ca.to_string_lossy().into_owned()) }, ..ClientConfig::default() })
            .expect("client");
    let response = runtime.block_on(client.send(RequestSpec::get(format!("{}/echo", server.base_url())), &Cancel::new())).expect("tls send");
    assert_eq!(response.status, 200);
    // The testkit advertises h2 first; the client must tell hyper-util that ALPN selected h2,
    // otherwise it writes HTTP/1.1 onto an h2 connection and fails with SendRequest.
    assert_eq!(response.version, hyper::Version::HTTP_2, "ALPN h2 is spoken as HTTP/2");
    assert!(response.timings.connect.tls_ms > 0, "TLS phase measured: {:?}", response.timings.connect);

    // WITHOUT the CA: the self-signed certificate is refused with a TLS-phase error.
    let strict = Client::new(ClientConfig::default()).expect("client");
    let err = runtime.block_on(strict.send(RequestSpec::get(format!("{}/echo", server.base_url())), &Cancel::new())).expect_err("must refuse");
    assert!(matches!(err, HttpError::Tls { .. }), "{err}");
    assert_eq!(err.phase(), "tls");

    // The explicit insecure toggle accepts it (and the caller knows: UI chip + console).
    let insecure = Client::new(ClientConfig { tls: TlsMode::Insecure, ..ClientConfig::default() }).expect("client");
    let response = runtime.block_on(insecure.send(RequestSpec::get(format!("{}/echo", server.base_url())), &Cancel::new())).expect("insecure send");
    assert_eq!(response.status, 200);
}

#[test]
fn a_timeout_aborts_and_reports_the_request_phase() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let spec = RequestSpec { timeout_ms: 80, ..RequestSpec::get(format!("{}/slow?ms=800", server.base_url())) };
    let started = std::time::Instant::now();
    let err = runtime.block_on(client.send(spec, &Cancel::new())).expect_err("timeout");
    assert!(matches!(err, HttpError::Timeout { millis: 80 }), "{err}");
    assert!(started.elapsed() < Duration::from_millis(600), "aborted promptly: {:?}", started.elapsed());
}

#[test]
fn cancellation_aborts_an_in_flight_request() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let spec = RequestSpec::get(format!("{}/slow?ms=800", server.base_url()));
    let cancel = Cancel::new();
    let trigger = cancel.clone();
    runtime.spawn(async move {
        tokio::time::sleep(Duration::from_millis(60)).await;
        trigger.cancel();
    });
    let started = std::time::Instant::now();
    let err = runtime.block_on(client.send(spec, &cancel)).expect_err("cancelled");
    assert!(matches!(err, HttpError::Cancelled), "{err}");
    assert!(started.elapsed() < Duration::from_millis(600), "cancelled promptly: {:?}", started.elapsed());
}

#[test]
fn the_body_cap_refuses_oversized_responses() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let spec = RequestSpec { max_body_bytes: 1024, ..RequestSpec::get(format!("{}/huge?mb=1", server.base_url())) };
    let err = runtime.block_on(client.send(spec, &Cancel::new())).expect_err("cap");
    assert!(matches!(err, HttpError::BodyTooLarge { cap: 1024 }), "{err}");
    assert_eq!(err.phase(), "download");
}

#[test]
fn compressed_responses_decode_with_the_size_breakdown() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    for kind in ["gzip", "deflate", "br", "zstd"] {
        let response = runtime
            .block_on(client.send(RequestSpec::get(format!("{}/compressed?kind={kind}&size=4096", server.base_url())), &Cancel::new()))
            .expect("send");
        assert!(response.headers.iter().any(|(name, value)| name == "content-encoding" && value == kind), "{kind}");
        assert_eq!(response.body.len(), 4096, "{kind} decoded to the pattern length");
        assert!(response.body.iter().all(u8::is_ascii_digit), "{kind} pattern bytes");
        assert_eq!(response.size.body_decoded_bytes, 4096);
        assert!(response.size.body_encoded_bytes < 4096 || kind == "zstd", "{kind} arrived compressed (raw-block zstd adds framing)");
        assert!(response.size.header_bytes > 0);
        assert!(response.size.content_encoding.is_some(), "{kind} encoding reported");
    }
    // Decoding OFF shows wire bytes and the encoded/decoded numbers agree.
    let raw_spec = RequestSpec {
        decode_compression: false,
        max_body_bytes: 1 << 20,
        ..RequestSpec::get(format!("{}/compressed?kind=gzip&size=4096", server.base_url()))
    };
    let raw = runtime.block_on(client.send(raw_spec, &Cancel::new())).expect("raw send");
    assert_eq!(&raw.body[..2], &[0x1F, 0x8B]);
    assert_eq!(raw.size.body_encoded_bytes, raw.size.body_decoded_bytes);
}

#[test]
fn send_to_file_streams_the_body_without_buffering_it() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("body.bin");
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let spec = RequestSpec::get(format!("{}/huge?mb=2", server.base_url()));
    let meta = runtime.block_on(client.send_to_file(spec, &Cancel::new(), &target)).expect("stream");
    assert_eq!(meta.status, 200);
    assert_eq!(meta.bytes_written, 2 * 1024 * 1024);
    let written = std::fs::read(&target).expect("file");
    assert_eq!(written.len(), 2 * 1024 * 1024);
    assert_eq!(written[0], b'0');
    assert!(meta.timings.total_ms >= meta.timings.ttfb_ms);
}

#[test]
fn http1_preference_forces_http1_on_a_dual_server() {
    let server = Server::start_tls();
    let dir = tempfile::tempdir().expect("tempdir");
    let ca = write_ca(dir.path(), server.ca_pem().expect("pem"));
    let runtime = runtime();
    let client = Client::new(ClientConfig {
        tls: TlsMode::Verify { ca_pem_path: Some(ca.to_string_lossy().into_owned()) },
        version: HttpVersionPreference::Http1,
        ..ClientConfig::default()
    })
    .expect("client");
    let response = runtime.block_on(client.send(RequestSpec::get(format!("{}/echo", server.base_url())), &Cancel::new())).expect("send");
    assert_eq!(response.version, hyper::Version::HTTP_11);
}

#[test]
fn a_built_request_arrives_in_order_with_multipart_bytes_intact() {
    use postguy_http::build::{BuildInput, build};
    use postguy_model::{Body, KeyValue, KeyValueKind};

    let server = Server::start();
    let runtime = runtime();
    let user_headers = vec![KeyValue { key: "X-Probe".into(), value: "alpha".into(), enabled: true, ..Default::default() }];
    let body =
        Body::FormData(vec![KeyValue { key: "field".into(), value: "value".into(), enabled: true, kind: KeyValueKind::Text, ..Default::default() }]);
    let built = build(BuildInput {
        method: "POST",
        url: &format!("{}/echo", server.base_url()),
        headers: &user_headers,
        body: &body,
        load_file: None,
        send_no_cache: true,
    })
    .expect("build");
    let client = Client::new(ClientConfig::default()).expect("client");
    let response = runtime.block_on(client.send(built.clone().into_spec(0, 1 << 20), &Cancel::new())).expect("send");
    assert_eq!(response.status, 200);
    let parsed: serde_json::Value = serde_json::from_slice(&response.body).expect("json");
    // Wire order: the user header first, then the automatic set in builder order.
    let names: Vec<&str> = parsed["headers"]
        .as_array()
        .expect("headers")
        .iter()
        .filter_map(|header| header["name"].as_str())
        .filter(|name| !name.starts_with("content-") && *name != "host" && *name != "content-length")
        .collect();
    assert_eq!(&names[..2], &["x-probe", "user-agent"], "{names:?}");
    // Names go lowercase on HTTP/1 (documented limit); multipart bytes arrive whole.
    let sent_body: String = parsed["body"]["utf8"].as_str().expect("utf8 body").to_string();
    assert!(sent_body.starts_with("--postguy-boundary-"), "{sent_body}");
    assert!(sent_body.contains("name=\"field\""));
    assert!(sent_body.trim_end().ends_with("--"));
}
