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
fn compressed_bytes_are_returned_raw_for_the_decode_layer() {
    let server = Server::start();
    let runtime = runtime();
    let client = Client::new(ClientConfig::default()).expect("client");
    let response = runtime
        .block_on(client.send(RequestSpec::get(format!("{}/compressed?kind=gzip&size=4096", server.base_url())), &Cancel::new()))
        .expect("send");
    assert!(response.headers.iter().any(|(name, value)| name == "content-encoding" && value == "gzip"));
    assert_eq!(&response.body[..2], &[0x1F, 0x8B], "raw gzip bytes reach the caller (decode is M2.4)");
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
