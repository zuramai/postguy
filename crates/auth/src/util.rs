//! Encoding and MAC helpers shared by the auth schemes.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest as _, Sha256, Sha384, Sha512, Sha512_256};

use crate::error::{AuthError, Result};

pub(crate) fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn b64std(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// RFC 3986 percent encoding: unreserved characters pass through, everything else is `%XX`.
pub(crate) fn pct(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacAlgorithm {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

pub(crate) fn hmac(algorithm: MacAlgorithm, key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    macro_rules! run {
        ($hash:ty) => {{
            let mut mac = <Hmac<$hash> as Mac>::new_from_slice(key).map_err(|err| AuthError::Invalid { kind: "hmac", reason: err.to_string() })?;
            mac.update(data);
            Ok(mac.finalize().into_bytes().to_vec())
        }};
    }
    match algorithm {
        MacAlgorithm::Sha1 => run!(Sha1),
        MacAlgorithm::Sha256 => run!(Sha256),
        MacAlgorithm::Sha384 => run!(Sha384),
        MacAlgorithm::Sha512 => run!(Sha512),
    }
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

pub(crate) fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

pub(crate) fn md5_hex(data: &[u8]) -> String {
    hex(&Md5::digest(data))
}

pub(crate) fn sha512_256_hex(data: &[u8]) -> String {
    hex(&Sha512_256::digest(data))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|duration| duration.as_secs()).unwrap_or(0)
}

/// OS CSPRNG bytes (OAuth state, PKCE verifiers, Digest/Hawk/OAuth1 nonces). Entropy failure
/// is an error, never a weak fallback.
pub(crate) fn random_hex(bytes: usize) -> Result<String> {
    let mut data = vec![0u8; bytes];
    getrandom::fill(&mut data).map_err(|err| AuthError::Invalid { kind: "entropy", reason: err.to_string() })?;
    Ok(hex(&data))
}

/// The parts auth schemes need from a URL (raw path, decoded query pairs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UrlParts {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
    pub query: Vec<(String, String)>,
}

impl UrlParts {
    pub fn parse(url: &str) -> Result<UrlParts> {
        let (scheme, rest) = url.split_once("://").ok_or_else(|| AuthError::Invalid { kind: "url", reason: "the URL has no scheme".into() })?;
        let (authority, remainder) = match rest.split_once('/') {
            Some((authority, path)) => (authority, format!("/{path}")),
            None => (rest, String::from("/")),
        };
        let authority = authority.rsplit_once('@').map(|(_, host)| host).unwrap_or(authority);
        let (host, port) = split_host_port(authority)?;
        let (path, query) = match remainder.split_once('?') {
            Some((path, query)) => (if path.is_empty() { "/".to_string() } else { path.to_string() }, query),
            None => (if remainder.is_empty() { "/".to_string() } else { remainder }, ""),
        };
        let path = path.split('#').next().unwrap_or("/").to_string();
        let query =
            postguy_urlkit::params_of_query(query.split('#').next().unwrap_or_default()).into_iter().map(|param| (param.key, param.value)).collect();
        Ok(UrlParts { scheme: scheme.to_ascii_lowercase(), host: host.to_ascii_lowercase(), port, path, query })
    }

    /// Host header form: include a non-default port.
    pub fn host_header(&self) -> String {
        match self.port.filter(|port| !default_port(&self.scheme, *port)) {
            Some(port) => format!("{}:{port}", self.host),
            None => self.host.clone(),
        }
    }

    /// Scheme://host[:port]/path, with default ports omitted.
    pub fn base_url(&self) -> String {
        let port = self.port.filter(|port| !default_port(&self.scheme, *port));
        match port {
            Some(port) => format!("{}://{}:{}{}", self.scheme, self.host, port, self.path),
            None => format!("{}://{}{}", self.scheme, self.host, self.path),
        }
    }
}

fn default_port(scheme: &str, port: u16) -> bool {
    (scheme == "http" && port == 80) || (scheme == "https" && port == 443)
}

fn split_host_port(authority: &str) -> Result<(String, Option<u16>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']').ok_or_else(|| AuthError::Invalid { kind: "url", reason: "unclosed IPv6 host".into() })?;
        let port = tail.strip_prefix(':').and_then(|port| port.parse().ok());
        return Ok((format!("[{host}]"), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.chars().all(|character| character.is_ascii_digit()) => {
            Ok((host.to_string(), port.parse().ok()))
        }
        _ => Ok((authority.to_string(), None)),
    }
}

/// Decode an `application/x-www-form-urlencoded` pair (`+` is a space).
pub(crate) fn form_decode(value: &str) -> String {
    postguy_urlkit::decode_percent(&value.replace('+', " "))
}

pub(crate) fn form_params(body: &[u8]) -> Vec<(String, String)> {
    let Ok(text) = std::str::from_utf8(body) else { return Vec::new() };
    text.split('&')
        .filter(|segment| !segment.is_empty())
        .map(|segment| match segment.split_once('=') {
            Some((key, value)) => (form_decode(key), form_decode(value)),
            None => (form_decode(segment), String::new()),
        })
        .collect()
}
