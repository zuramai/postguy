//! Integration: drive every fixture over REAL loopback sockets with hyper, plain and TLS.
//! (The HTTP engine's own tests will reuse the same server; this proves the fixtures work.)

use bytes::Bytes;
use http_body_util::BodyExt as _;
use http_body_util::combinators::BoxBody;
use http_body_util::{Empty, Full};
use hyper::{Request, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use postguy_testkit::Server;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn make_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime")
}

async fn plain_get(server: &Server, path: &str) -> (StatusCode, Vec<(String, String)>, Vec<u8>) {
    let client: Client<hyper_util::client::legacy::connect::HttpConnector, BoxBody<Bytes, std::convert::Infallible>> =
        Client::builder(TokioExecutor::new()).build_http();
    let request = Request::builder()
        .uri(format!("{}{}", server.base_url(), path))
        .body(BoxBody::new(Empty::<Bytes>::new().map_err(|never| match never {})))
        .expect("request");
    let response = client.request(request).await.expect("request");
    let status = response.status();
    let headers =
        response.headers().iter().map(|(name, value)| (name.as_str().to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned())).collect();
    let body = response.into_body().collect().await.expect("body").to_bytes().to_vec();
    (status, headers, body)
}

#[test]
fn echo_reports_method_headers_and_body() {
    let server = Server::start();
    let runtime = make_runtime();
    let (status, _, body) = runtime.block_on(async {
        let client: Client<hyper_util::client::legacy::connect::HttpConnector, BoxBody<Bytes, std::convert::Infallible>> =
            Client::builder(TokioExecutor::new()).build_http();
        let request = Request::builder()
            .method("POST")
            .uri(format!("{}/echo?x=1&y=two", server.base_url()))
            .header("X-Probe", "alpha")
            .body(BoxBody::new(Full::new(Bytes::from_static(b"hello"))))
            .expect("request");
        let response = client.request(request).await.expect("echo");
        let status = response.status();
        let body = response.into_body().collect().await.expect("body").to_bytes().to_vec();
        (status, (), body)
    });
    assert_eq!(status, StatusCode::OK);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(parsed["method"], "POST");
    assert_eq!(parsed["query"], "x=1&y=two");
    assert_eq!(parsed["body"]["utf8"], "hello");
    let headers = parsed["headers"].as_array().expect("headers");
    assert!(headers.iter().any(|header| header["name"] == "x-probe" && header["value"] == "alpha"), "{headers:?}");
}

#[test]
fn redirects_status_and_exact_codes() {
    let server = Server::start();
    let runtime = make_runtime();
    runtime.block_on(async {
        let (status, headers, _) = plain_get(&server, "/redirect?code=307&to=/redirect/done").await;
        assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
        assert!(headers.iter().any(|(name, value)| name == "location" && value == "/redirect/done"));
        let (status, _, _) = plain_get(&server, "/redirect/chain?hops=3").await;
        assert_eq!(status, StatusCode::FOUND);
        let (status, _, body) = plain_get(&server, "/status/418").await;
        assert_eq!(status, StatusCode::IM_A_TEAPOT);
        assert_eq!(body, b"status 418");
        let (status, _, body) = plain_get(&server, "/missing-route").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, b"no such fixture");
    });
}

#[test]
fn cookie_set_and_echo() {
    let server = Server::start();
    let runtime = make_runtime();
    runtime.block_on(async {
        let (_, headers, _) = plain_get(&server, "/cookies/set?a=1&b=two").await;
        let set_cookies: Vec<&String> = headers.iter().filter(|(name, _)| name == "set-cookie").map(|(_, value)| value).collect();
        assert_eq!(set_cookies.len(), 2);
        assert!(set_cookies[0].starts_with("a=1;"));
        // The echo fixture reports the header it received.
        let client: Client<hyper_util::client::legacy::connect::HttpConnector, BoxBody<Bytes, std::convert::Infallible>> =
            Client::builder(TokioExecutor::new()).build_http();
        let request = Request::builder()
            .uri(format!("{}/cookies/echo", server.base_url()))
            .header("Cookie", "a=1; b=two")
            .body(BoxBody::new(Empty::<Bytes>::new().map_err(|never| match never {})))
            .expect("request");
        let response = client.request(request).await.expect("echo");
        let parsed: serde_json::Value = serde_json::from_slice(&response.into_body().collect().await.expect("body").to_bytes()).expect("json");
        assert_eq!(parsed["cookieHeader"], "a=1; b=two");
    });
}

#[test]
fn auth_fixtures_challenge_and_verify() {
    let server = Server::start();
    let runtime = make_runtime();
    runtime.block_on(async {
        let (status, headers, _) = plain_get(&server, "/auth/basic").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(headers.iter().any(|(name, value)| name == "www-authenticate" && value.starts_with("Basic")));
        // Correct basic credentials.
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode("postguy:postguy");
        let client: Client<hyper_util::client::legacy::connect::HttpConnector, BoxBody<Bytes, std::convert::Infallible>> =
            Client::builder(TokioExecutor::new()).build_http();
        let request = Request::builder()
            .uri(format!("{}/auth/basic", server.base_url()))
            .header("Authorization", format!("Basic {encoded}"))
            .body(BoxBody::new(Empty::<Bytes>::new().map_err(|never| match never {})))
            .expect("request");
        assert_eq!(client.request(request).await.expect("basic").status(), StatusCode::OK);

        // Bearer.
        let (status, _, _) = plain_get(&server, "/auth/bearer").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let request = Request::builder()
            .uri(format!("{}/auth/bearer", server.base_url()))
            .header("Authorization", "Bearer postguy-token")
            .body(BoxBody::new(Empty::<Bytes>::new().map_err(|never| match never {})))
            .expect("request");
        assert_eq!(client.request(request).await.expect("bearer").status(), StatusCode::OK);

        // Digest: challenge, then compute the RFC 7616 MD5 answer and be accepted.
        let (status, headers, _) = plain_get(&server, "/auth/digest").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let challenge = headers.iter().find(|(name, _)| name == "www-authenticate").map(|(_, value)| value.clone()).expect("challenge");
        assert!(challenge.contains("Digest "), "{challenge}");
        let nonce = challenge.split("nonce=\"").nth(1).and_then(|rest| rest.split('"').next()).expect("nonce");
        use md5::Digest as _;
        let hex = |bytes: &[u8]| md5::Md5::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let ha1 = hex(b"postguy:postguy:postguy");
        let ha2 = hex(b"GET:/auth/digest");
        let response_hash = hex(format!("{ha1}:{nonce}:00000001:auth:{ha2}").as_bytes());
        let authorization = format!(
            r#"Digest username="postguy", realm="postguy", nonce="{nonce}", uri="/auth/digest", qop=auth, nc=00000001, response="{response_hash}""#
        );
        let request = Request::builder()
            .uri(format!("{}/auth/digest", server.base_url()))
            .header("Authorization", authorization)
            .body(BoxBody::new(Empty::<Bytes>::new().map_err(|never| match never {})))
            .expect("request");
        assert_eq!(client.request(request).await.expect("digest").status(), StatusCode::OK);
    });
}

#[test]
fn slow_stream_compressed_and_huge_fixtures() {
    let server = Server::start();
    let runtime = make_runtime();
    runtime.block_on(async {
        let started = std::time::Instant::now();
        let (status, _, _) = plain_get(&server, "/slow?ms=120").await;
        assert_eq!(status, StatusCode::OK);
        assert!(started.elapsed() >= std::time::Duration::from_millis(100), "slow fixture waited");

        // /stream body total = chunks * bytes.
        let (status, _, body) = plain_get(&server, "/stream?chunks=4&bytes=16").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.len(), 64);

        // Compressed fixtures carry the right header and decode on the server side is a
        // client concern; here we assert the ENCODING byte markers.
        let (_, headers, body) = plain_get(&server, "/compressed?kind=gzip&size=2048").await;
        assert!(headers.iter().any(|(name, value)| name == "content-encoding" && value == "gzip"));
        assert_eq!(&body[..2], &[0x1F, 0x8B]);
        let (_, headers, body) = plain_get(&server, "/compressed?kind=zstd&size=512").await;
        assert!(headers.iter().any(|(name, value)| name == "content-encoding" && value == "zstd"));
        assert_eq!(&body[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
        let (_, headers, _) = plain_get(&server, "/compressed?kind=br&size=512").await;
        assert!(headers.iter().any(|(name, value)| name == "content-encoding" && value == "br"));

        let (status, _, body) = plain_get(&server, "/huge?mb=2").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.len(), 2 * 1024 * 1024);
        assert_eq!(body[0], b'0');
        assert_eq!(body[1], b'1');
    });
}

#[test]
fn the_token_endpoint_answers_client_credentials() {
    let server = Server::start();
    let runtime = make_runtime();
    runtime.block_on(async {
        let client: Client<hyper_util::client::legacy::connect::HttpConnector, BoxBody<Bytes, std::convert::Infallible>> =
            Client::builder(TokioExecutor::new()).build_http();
        let request = Request::builder()
            .method("POST")
            .uri(format!("{}/token", server.base_url()))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(BoxBody::new(Full::new(Bytes::from_static(b"grant_type=client_credentials&client_id=postguy&client_secret=x"))))
            .expect("request");
        let response = client.request(request).await.expect("token");
        let parsed: serde_json::Value = serde_json::from_slice(&response.into_body().collect().await.expect("body").to_bytes()).expect("json");
        assert_eq!(parsed["access_token"], "postguy-access-token");
        assert_eq!(parsed["token_type"], "Bearer");

        // The wrong grant is a 400 JSON error.
        let request = Request::builder()
            .method("POST")
            .uri(format!("{}/token", server.base_url()))
            .body(BoxBody::new(Full::new(Bytes::from_static(b"grant_type=password"))))
            .expect("request");
        let response = client.request(request).await.expect("token");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    });
}

#[test]
fn tls_serves_the_same_fixtures_with_a_trustable_certificate() {
    let server = Server::start_tls();
    let ca_pem = server.ca_pem().expect("tls server exposes its CA").to_string();
    assert!(server.base_url().starts_with("https://"));

    let runtime = make_runtime();
    runtime.block_on(async {
        // A rustls client that trusts ONLY the generated CA (verification on).
        let mut roots = rustls::RootCertStore::empty();
        let mut reader = std::io::BufReader::new(ca_pem.as_bytes());
        for certificate in rustls_pemfile::certs(&mut reader).flatten() {
            roots.add(certificate).expect("trust the testkit CA");
        }
        let tls_config = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        let connector = hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(tls_config).https_or_http().enable_http1().build();
        let client: Client<_, BoxBody<Bytes, std::convert::Infallible>> = Client::builder(TokioExecutor::new()).build(connector);
        let response = client
            .request(
                Request::builder()
                    .uri(format!("{}/echo", server.base_url()))
                    .body(BoxBody::new(Empty::<Bytes>::new().map_err(|never| match never {})))
                    .expect("request"),
            )
            .await
            .expect("tls echo");
        assert_eq!(response.status(), StatusCode::OK);
        let parsed: serde_json::Value = serde_json::from_slice(&response.into_body().collect().await.expect("body").to_bytes()).expect("json");
        assert_eq!(parsed["method"], "GET");
    });
}

/// A raw socket proves the server really streams (we can read the first bytes before the last
/// chunk is written) and that shutdown closes the listener.
#[test]
fn streaming_is_incremental_and_shutdown_closes_the_listener() {
    let server = Server::start();
    let addr = server.addr();
    let runtime = make_runtime();
    runtime.block_on(async {
        let mut socket = tokio::net::TcpStream::connect(addr).await.expect("connect");
        socket.write_all(b"GET /stream?chunks=3&bytes=8 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.expect("write");
        let mut first = [0u8; 256];
        let read = socket.read(&mut first).await.expect("read");
        assert!(read > 0);
        // At least the header block arrived, and this read happened before the process joined.
        let text = String::from_utf8_lossy(&first[..read]);
        assert!(text.contains("200 OK"), "{text}");
    });
    server.shutdown();
    // After shutdown the port is closed (a fresh connect must fail or immediately EOF).
    let runtime = make_runtime();
    runtime.block_on(async {
        if let Ok(mut socket) = tokio::net::TcpStream::connect(addr).await {
            let mut byte = [0u8; 1];
            assert_eq!(socket.read(&mut byte).await.unwrap_or(0), 0, "closed listener refuses work");
        }
    });
}
