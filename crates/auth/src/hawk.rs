//! Hawk authentication (Mozilla's Hawk 1.0): HMAC-SHA256/SHA1 over the normalized request
//! string, with optional ext/app/dlg and payload hash. Header generation only.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use postguy_model::auth::{HawkAlgorithm, HawkAuth};

use crate::error::{Result, invalid, missing};
use crate::util::{MacAlgorithm, UrlParts, b64std, hmac, random_hex, sha256};

/// Build the `Authorization: Hawk ...` value.
pub fn authorization(auth: &HawkAuth, method: &str, url: &str, body: &[u8], now: u64) -> Result<String> {
    if auth.auth_id.is_empty() {
        return Err(missing("hawk", "auth id"));
    }
    if auth.auth_key.is_empty() {
        return Err(missing("hawk", "auth key"));
    }
    let parts = UrlParts::parse(url)?;
    let timestamp = if auth.timestamp.trim().is_empty() { now.to_string() } else { auth.timestamp.trim().to_string() };
    timestamp.parse::<u64>().map_err(|_| invalid("hawk", "timestamp must be unix seconds"))?;
    let nonce = if auth.nonce.trim().is_empty() { random_hex(6)? } else { auth.nonce.clone() };
    let path = match parts.query.is_empty() {
        true => parts.path.clone(),
        false => format!(
            "{}?{}",
            parts.path,
            parts.query.iter().map(|(key, value)| format!("{}={}", crate::util::pct(key), crate::util::pct(value))).collect::<Vec<_>>().join("&")
        ),
    };
    let hash = if auth.include_payload_hash { STANDARD.encode(sha256(body)) } else { String::new() };
    let port = parts.port.map(|port| port.to_string()).unwrap_or_default();
    let mut normalized =
        format!("hawk.1.header\n{timestamp}\n{nonce}\n{}\n{path}\n{}\n{port}\n{hash}\n{}\n", method.to_ascii_uppercase(), parts.host, auth.ext);
    if !auth.app_id.is_empty() {
        normalized.push_str(&format!("{}\n", auth.app_id));
    }
    if !auth.dlg.is_empty() {
        normalized.push_str(&format!("{}\n", auth.dlg));
    }
    let algorithm = match auth.algorithm {
        HawkAlgorithm::Sha256 => MacAlgorithm::Sha256,
        HawkAlgorithm::Sha1 => MacAlgorithm::Sha1,
    };
    let mac = b64std(&hmac(algorithm, auth.auth_key.as_bytes(), normalized.as_bytes())?);
    let mut fields = vec![
        format!("id=\"{}\"", auth.auth_id.replace('"', "\\\"")),
        format!("ts=\"{timestamp}\""),
        format!("nonce=\"{}\"", nonce.replace('"', "\\\"")),
    ];
    if !auth.ext.is_empty() {
        fields.push(format!("ext=\"{}\"", auth.ext.replace('"', "\\\"")));
    }
    if !auth.app_id.is_empty() {
        fields.push(format!("app=\"{}\"", auth.app_id.replace('"', "\\\"")));
    }
    if !auth.dlg.is_empty() {
        fields.push(format!("dlg=\"{}\"", auth.dlg.replace('"', "\\\"")));
    }
    fields.push(format!("mac=\"{mac}\""));
    Ok(format!("Hawk {}", fields.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hawk_readme_vector_matches() {
        let auth = HawkAuth {
            auth_id: "dh37fgj492je".into(),
            auth_key: "werxhqb98rpaxn39848xrunpaw3489ruxnpa98w4rxn".into(),
            algorithm: HawkAlgorithm::Sha256,
            ext: "some-app-ext-data".into(),
            timestamp: "1353832234".into(),
            nonce: "j4h3g2".into(),
            ..HawkAuth::default()
        };
        let value = authorization(&auth, "GET", "http://example.com:8000/resource/1?b=1&a=2", b"", 0).expect("hawk");
        assert!(value.contains("mac=\"6R4rV5iE+NPoym+WwjeHzjAGXUtLNIxmo1vpMofpLAE=\""), "{value}");
        assert!(value.contains("ext=\"some-app-ext-data\""));
    }

    #[test]
    fn missing_credentials_are_actionable() {
        let error = authorization(&HawkAuth::default(), "GET", "https://example.test/", b"", 0).expect_err("missing");
        assert!(error.to_string().contains("auth id"), "{error}");
    }
}
