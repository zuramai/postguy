//! The route table: every fixture the HTTP engine milestone needs, dispatched on path.
//!
//! | Path | Behaviour |
//! |---|---|
//! | `/echo` | JSON of method, path, query, headers in received order, body (UTF-8 or base64) |
//! | `/status/<code>` | that status with a one-line body |
//! | `/redirect?to=/target&code=302` | Location redirect (default 302) |
//! | `/redirect/chain?hops=N` | first of an N-hop chain ending at `/redirect/done` |
//! | `/redirect/done` | 200, marks chain end |
//! | `/cookies/set?name=value&...` | one Set-Cookie per parameter (with Path=/) |
//! | `/cookies/echo` | JSON of the Cookie header as received |
//! | `/auth/basic` | 401 + WWW-Authenticate: Basic unless correct credentials |
//! | `/auth/bearer` | 401 unless `Authorization: Bearer postguy-token` |
//! | `/auth/digest` | 401 with a Digest challenge; verifies the RFC 7616 answer (MD5) |
//! | `/slow?ms=N` | waits N ms, then 200 |
//! | `/stream?chunks=N&bytes=M` | chunked response, one flush per chunk |
//! | `/compressed?kind=gzip|deflate|br|zstd&size=N[&truncate=1]` | compressed pattern body |
//! | `/huge?mb=N` | N MiB pattern body |
//! | `/token` (POST) | OAuth 2.0 token endpoint: grant_type=client_credentials → JSON token |
//! | `/no-content` | 204 with no body |

use base64::Engine as _;
use hyper::header::{CONTENT_TYPE, LOCATION, SET_COOKIE, WWW_AUTHENTICATE};
use hyper::{Method, StatusCode};
use serde_json::{Value, json};

use crate::body;

/// The server's answer: status, headers and the body bytes, plus an optional stream of extra
/// chunks (the server writes them with flushes between).
pub struct Reply {
    pub status: StatusCode,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub chunks: Vec<Vec<u8>>,
    pub delay_ms: u64,
}

impl Reply {
    pub fn ok(body: Vec<u8>) -> Reply {
        Reply { status: StatusCode::OK, headers: Vec::new(), body, chunks: Vec::new(), delay_ms: 0 }
    }

    pub fn with_header(mut self, name: &str, value: impl Into<String>) -> Reply {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    pub fn with_json(value: &Value) -> Reply {
        Reply::ok(value.to_string().into_bytes()).with_header(CONTENT_TYPE.as_str(), "application/json")
    }
}

/// The request as the routes need it (raw pieces, order preserved).
pub struct Incoming {
    pub method: Method,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Run one request through the route table.
pub fn route(request: Incoming) -> Reply {
    let path = request.path.as_str();
    match path {
        "/echo" => echo(&request),
        "/redirect" => redirect(&request.query),
        "/redirect/chain" => redirect_chain(&request.query),
        "/redirect/loop" => redirect_loop(),
        "/redirect/done" => Reply::ok(b"done".to_vec()),
        "/cookies/set" => cookies_set(&request.query),
        "/cookies/redirect-set" => cookies_redirect_set(&request.query),
        "/cookies/echo" => cookies_echo(&request),
        "/auth/basic" => auth_basic(&request),
        "/auth/bearer" => auth_bearer(&request),
        "/auth/digest" => auth_digest(&request),
        "/slow" => slow(&request.query),
        "/stream" => stream(&request.query),
        "/compressed" => compressed(&request.query),
        "/huge" => huge(&request.query),
        "/token" => token(&request),
        "/no-content" => Reply { status: StatusCode::NO_CONTENT, headers: Vec::new(), body: Vec::new(), chunks: Vec::new(), delay_ms: 0 },
        _ => {
            if let Some(code) = path.strip_prefix("/status/").and_then(|code| code.parse::<u16>().ok())
                && let Ok(status) = StatusCode::from_u16(code)
            {
                return Reply::ok(format!("status {code}").into_bytes()).with_header(CONTENT_TYPE.as_str(), "text/plain").tap_status(status);
            }
            Reply { status: StatusCode::NOT_FOUND, headers: Vec::new(), body: b"no such fixture".to_vec(), chunks: Vec::new(), delay_ms: 0 }
        }
    }
}

fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|segment| !segment.is_empty())
        .map(|segment| match segment.split_once('=') {
            Some((key, value)) => (decode(key), decode(value)),
            None => (decode(segment), String::new()),
        })
        .collect()
}

fn query_get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(candidate, _)| candidate == key).map(|(_, value)| value.as_str())
}

fn query_usize(pairs: &[(String, String)], key: &str, default: usize) -> usize {
    query_get(pairs, key).and_then(|value| value.parse().ok()).unwrap_or(default)
}

/// Minimal percent-decoding for fixture query values (matches the client's percent encoding).
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && bytes.get(index + 1).and_then(|b| char::from(*b).to_digit(16)).is_some()
            && bytes.get(index + 2).and_then(|b| char::from(*b).to_digit(16)).is_some()
        {
            let high = char::from(bytes[index + 1]).to_digit(16).unwrap_or(0) as u8;
            let low = char::from(bytes[index + 2]).to_digit(16).unwrap_or(0) as u8;
            out.push((high << 4) | low);
            index += 3;
        } else if bytes[index] == b'+' {
            out.push(b' ');
            index += 1;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn echo(request: &Incoming) -> Reply {
    let body_text = match String::from_utf8(request.body.clone()) {
        Ok(text) => json!({"utf8": text}),
        Err(_) => json!({"base64": base64::engine::general_purpose::STANDARD.encode(&request.body)}),
    };
    Reply::with_json(&json!({
        "method": request.method.as_str(),
        "path": request.path,
        "query": request.query,
        "headers": request.headers.iter().map(|(name, value)| json!({"name": name, "value": value})).collect::<Vec<_>>(),
        "body": body_text,
        "bodyLength": request.body.len(),
    }))
}

fn redirect(query: &str) -> Reply {
    let pairs = query_pairs(query);
    let code = query_get(&pairs, "code").and_then(|value| value.parse::<u16>().ok()).unwrap_or(302);
    let to = query_get(&pairs, "to").unwrap_or("/redirect/done");
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::FOUND);
    Reply::ok(Vec::new()).with_header(LOCATION.as_str(), to).tap_status(status)
}

trait TapStatus {
    fn tap_status(self, status: StatusCode) -> Reply;
}

impl TapStatus for Reply {
    fn tap_status(mut self, status: StatusCode) -> Reply {
        self.status = status;
        self
    }
}

fn redirect_chain(query: &str) -> Reply {
    let pairs = query_pairs(query);
    let hops = query_usize(&pairs, "hops", 1).max(1);
    let to = query_get(&pairs, "to").unwrap_or("/redirect/done");
    let next = if hops <= 1 { to.to_string() } else { format!("/redirect/chain?hops={}&to={}", hops - 1, encode_fixture(to)) };
    Reply::ok(Vec::new()).tap_status(StatusCode::FOUND).with_header(LOCATION.as_str(), &next)
}

fn redirect_loop() -> Reply {
    Reply::ok(Vec::new()).tap_status(StatusCode::FOUND).with_header(LOCATION.as_str(), "/redirect/loop")
}

/// Minimal percent-encoding for fixture query propagation.
fn encode_fixture(value: &str) -> String {
    value.replace('%', "%25").replace('&', "%26").replace(' ', "%20")
}

fn cookies_set(query: &str) -> Reply {
    let mut reply = Reply::with_json(&json!({"set": true}));
    for (name, value) in query_pairs(query) {
        reply.headers.push((SET_COOKIE.as_str().to_string(), format!("{name}={value}; Path=/")));
    }
    reply
}

fn cookies_redirect_set(query: &str) -> Reply {
    let pairs = query_pairs(query);
    let to = query_get(&pairs, "to").unwrap_or("/cookies/echo");
    let name = query_get(&pairs, "name").unwrap_or("hop");
    let value = query_get(&pairs, "value").unwrap_or("1");
    Reply::ok(Vec::new())
        .tap_status(StatusCode::FOUND)
        .with_header(SET_COOKIE.as_str(), format!("{name}={value}; Path=/"))
        .with_header(LOCATION.as_str(), to)
}

fn cookies_echo(request: &Incoming) -> Reply {
    let cookies: Vec<&str> =
        request.headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case("cookie")).map(|(_, value)| value.as_str()).collect();
    Reply::with_json(&json!({"cookieHeader": cookies.join("; ")}))
}

fn header_value<'a>(request: &'a Incoming, name: &str) -> Option<&'a str> {
    request.headers.iter().find(|(candidate, _)| candidate.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
}

fn auth_basic(request: &Incoming) -> Reply {
    let expected = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("postguy:postguy"));
    match header_value(request, "authorization") {
        Some(value) if value == expected => Reply::with_json(&json!({"authenticated": "basic"})),
        _ => Reply::ok(b"auth required".to_vec())
            .tap_status(StatusCode::UNAUTHORIZED)
            .with_header(WWW_AUTHENTICATE.as_str(), r#"Basic realm="testkit""#),
    }
}

fn auth_bearer(request: &Incoming) -> Reply {
    match header_value(request, "authorization") {
        Some("Bearer postguy-token") => Reply::with_json(&json!({"authenticated": "bearer"})),
        _ => Reply::ok(b"auth required".to_vec()).tap_status(StatusCode::UNAUTHORIZED).with_header(WWW_AUTHENTICATE.as_str(), "Bearer"),
    }
}

/// Digest challenge/response, RFC 7616 with MD5 and `qop=auth` (the M2.7/5.1 path). The
/// challenge nonce is fixed ("postguy-nonce") so tests are deterministic; the response must be
/// the correct MD5 digest over the method and URI.
fn auth_digest(request: &Incoming) -> Reply {
    const REALM: &str = "postguy";
    const NONCE: &str = "postguy-nonce";
    const USERNAME: &str = "postguy";
    const PASSWORD: &str = "postguy";
    let Some(header) = header_value(request, "authorization") else {
        return digest_challenge(REALM, NONCE, "auth");
    };
    let Some(parameters) = parse_digest_parameters(header) else {
        return digest_challenge(REALM, NONCE, "auth");
    };
    let get = |key: &str| parameters.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value.as_str());
    let Some(response) = get("response") else { return digest_challenge(REALM, NONCE, "auth") };
    let uri = get("uri").unwrap_or_default();
    let qop = get("qop").unwrap_or_default();
    let nc = get("nc").unwrap_or_default();
    let algorithm = get("algorithm").unwrap_or("MD5");
    if !algorithm.eq_ignore_ascii_case("MD5") || !qop.eq_ignore_ascii_case("auth") || get("nonce").unwrap_or_default() != NONCE {
        return digest_challenge(REALM, NONCE, "auth");
    }
    let ha1 = hex_md5(format!("{USERNAME}:{REALM}:{PASSWORD}").as_bytes());
    let ha2 = hex_md5(format!("{}:{uri}", request.method.as_str()).as_bytes());
    let expected = hex_md5(format!("{ha1}:{NONCE}:{nc}:{qop}:{ha2}").as_bytes());
    if response.eq_ignore_ascii_case(&expected) {
        Reply::with_json(&json!({"authenticated": "digest"}))
    } else {
        Reply::ok(b"bad digest".to_vec())
            .tap_status(StatusCode::UNAUTHORIZED)
            .with_header(WWW_AUTHENTICATE.as_str(), format!(r#"Digest realm="{REALM}", qop="auth", nonce="{NONCE}", algorithm=MD5"#))
    }
}

fn digest_challenge(realm: &str, nonce: &str, qop: &str) -> Reply {
    Reply::ok(b"auth required".to_vec())
        .tap_status(StatusCode::UNAUTHORIZED)
        .with_header(WWW_AUTHENTICATE.as_str(), format!(r#"Digest realm="{realm}", qop="{qop}", nonce="{nonce}", algorithm=MD5"#))
}

fn parse_digest_parameters(header: &str) -> Option<Vec<(String, String)>> {
    let rest = header.strip_prefix("Digest ").or_else(|| header.strip_prefix("digest "))?;
    let mut parameters = Vec::new();
    for segment in rest.split(',') {
        let (key, value) = segment.trim().split_once('=')?;
        parameters.push((key.trim().to_string(), value.trim().trim_matches('"').to_string()));
    }
    Some(parameters)
}

fn hex_md5(bytes: &[u8]) -> String {
    use md5::Digest as _;
    md5::Md5::digest(bytes).iter().fold(String::with_capacity(32), |mut out, byte| {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn slow(query: &str) -> Reply {
    let pairs = query_pairs(query);
    let mut reply = Reply::with_json(&json!({"slow": true}));
    reply.delay_ms = query_usize(&pairs, "ms", 100) as u64;
    reply
}

fn stream(query: &str) -> Reply {
    let pairs = query_pairs(query);
    let chunks = query_usize(&pairs, "chunks", 5).min(10_000);
    let bytes = query_usize(&pairs, "bytes", 64).min(1 << 20);
    let mut reply = Reply::ok(Vec::new()).with_header(CONTENT_TYPE.as_str(), "application/octet-stream");
    reply.chunks = body::stream_chunks(chunks, bytes);
    reply
}

fn compressed(query: &str) -> Reply {
    let pairs = query_pairs(query);
    let kind = query_get(&pairs, "kind").unwrap_or("gzip");
    let size = query_usize(&pairs, "size", 1024).min(64 << 20);
    let payload = body::pattern(size);
    let (encoding, encoded) = match kind {
        "gzip" => ("gzip", body::gzip(&payload)),
        "deflate" => ("deflate", body::deflate(&payload)),
        "br" => ("br", body::brotli(&payload)),
        "zstd" => ("zstd", body::zstd_raw(&payload)),
        _ => ("identity", payload.clone()),
    };
    let mut reply = Reply::ok(encoded);
    if encoding != "identity" {
        reply = reply.with_header(hyper::header::CONTENT_ENCODING.as_str(), encoding);
    }
    reply = reply.with_header(hyper::header::CONTENT_TYPE.as_str(), "text/plain");
    if query_usize(&pairs, "truncate", 0) == 1 {
        // Claim a larger body than sent: the client's decode/size handling must fail cleanly.
        reply = reply.with_header(hyper::header::CONTENT_LENGTH.as_str(), "999999999");
    }
    reply
}

fn huge(query: &str) -> Reply {
    let pairs = query_pairs(query);
    let mb = query_usize(&pairs, "mb", 1).min(256);
    Reply::ok(body::pattern(mb * 1024 * 1024)).with_header(CONTENT_TYPE.as_str(), "application/octet-stream")
}

fn token(request: &Incoming) -> Reply {
    if request.method != Method::POST {
        return Reply::ok(b"POST required".to_vec()).tap_status(StatusCode::METHOD_NOT_ALLOWED);
    }
    let form = String::from_utf8_lossy(&request.body).into_owned();
    let pairs = query_pairs(&form);
    if query_get(&pairs, "grant_type") != Some("client_credentials") {
        return Reply::with_json(&json!({"error": "unsupported_grant_type"})).tap_status(StatusCode::BAD_REQUEST);
    }
    let client_id = query_get(&pairs, "client_id").unwrap_or("postguy");
    Reply::with_json(&json!({
        "access_token": "postguy-access-token",
        "token_type": "Bearer",
        "expires_in": 3600,
        "refresh_token": "postguy-refresh-token",
        "scope": "read write",
        "client_id": client_id,
    }))
}
