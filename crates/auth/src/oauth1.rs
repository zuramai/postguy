//! OAuth 1.0 request signing (RFC 5849): HMAC-SHA1/256/512, RSA-SHA1/256/512 and PLAINTEXT.
//! The same canonical parameter normalization produces the header fields or query additions.

use postguy_model::auth::{OAuth1Auth, OAuth1SignatureMethod};
use rsa::pkcs1::DecodeRsaPrivateKey as _;
use rsa::pkcs8::DecodePrivateKey as _;
use rsa::{Pkcs1v15Sign, RsaPrivateKey};
use sha1::Sha1;
use sha2::{Digest as _, Sha256, Sha512};

use crate::error::{AuthError, Result, invalid, missing};
use crate::util::{MacAlgorithm, UrlParts, b64std, form_params, hmac, pct};

/// What the caller must place on the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuth1Output {
    /// `Authorization: OAuth ...`, when adding params to the header.
    pub header: Option<String>,
    /// Otherwise these percent-encoded pairs are appended to the URL query.
    pub query: Vec<(String, String)>,
}

/// Sign a request. `body` is only consulted for form-urlencoded bodies and the body hash.
pub fn sign(auth: &OAuth1Auth, method: &str, url: &str, body: &[u8], timestamp: u64, nonce: &str) -> Result<OAuth1Output> {
    if auth.consumer_key.is_empty() {
        return Err(missing("oauth1", "consumer key"));
    }
    let parts = UrlParts::parse(url)?;
    let timestamp = if auth.timestamp.trim().is_empty() {
        timestamp
    } else {
        auth.timestamp.trim().parse::<u64>().map_err(|_| invalid("oauth1", "timestamp must be unix seconds"))?
    };
    let signature_method = auth.signature_method;
    let method_name = signature_name(signature_method);
    let mut oauth = vec![
        ("oauth_consumer_key".to_string(), auth.consumer_key.clone()),
        ("oauth_nonce".to_string(), nonce.to_string()),
        ("oauth_signature_method".to_string(), method_name.to_string()),
        ("oauth_timestamp".to_string(), timestamp.to_string()),
    ];
    if !auth.version.is_empty() {
        oauth.push(("oauth_version".to_string(), auth.version.clone()));
    }
    if !auth.token.is_empty() {
        oauth.push(("oauth_token".to_string(), auth.token.clone()));
    }
    if !auth.callback_url.is_empty() {
        oauth.push(("oauth_callback".to_string(), auth.callback_url.clone()));
    }
    if !auth.verifier.is_empty() {
        oauth.push(("oauth_verifier".to_string(), auth.verifier.clone()));
    }
    if auth.include_body_hash && !body.is_empty() {
        oauth.push(("oauth_body_hash".to_string(), b64std(&sha1_digest(body))));
    }

    // Query + form body parameters participate in the signature base string.
    let mut parameters: Vec<(String, String)> = parts.query.clone();
    if is_form_body(auth, body) {
        parameters.extend(form_params(body));
    }
    if auth.add_empty_params_to_sign {
        // Nothing to add: empty rows from the URL/body are included by default.
    }
    parameters.extend(oauth.iter().cloned());
    parameters.sort_by(|left, right| pct(&left.0).cmp(&pct(&right.0)).then_with(|| pct(&left.1).cmp(&pct(&right.1))));
    let normalized: Vec<String> = parameters.iter().map(|(key, value)| format!("{}={}", pct(key), pct(value))).collect();
    let base_string = format!("{}&{}&{}", method.to_ascii_uppercase(), pct(&parts.base_url()), pct(&normalized.join("&")));
    let signature = match signature_method {
        OAuth1SignatureMethod::Plaintext => format!("{}&{}", pct(&auth.consumer_secret), pct(&auth.token_secret)),
        OAuth1SignatureMethod::HmacSha1 | OAuth1SignatureMethod::HmacSha256 | OAuth1SignatureMethod::HmacSha512 => {
            let algorithm = match signature_method {
                OAuth1SignatureMethod::HmacSha1 => MacAlgorithm::Sha1,
                OAuth1SignatureMethod::HmacSha256 => MacAlgorithm::Sha256,
                _ => MacAlgorithm::Sha512,
            };
            let key = format!("{}&{}", pct(&auth.consumer_secret), pct(&auth.token_secret));
            b64std(&hmac(algorithm, key.as_bytes(), base_string.as_bytes())?)
        }
        OAuth1SignatureMethod::RsaSha1 | OAuth1SignatureMethod::RsaSha256 | OAuth1SignatureMethod::RsaSha512 => {
            let key = rsa_key(auth)?;
            let (padding, digest) = match signature_method {
                OAuth1SignatureMethod::RsaSha1 => (Pkcs1v15Sign::new::<Sha1>(), Sha1::digest(base_string.as_bytes()).to_vec()),
                OAuth1SignatureMethod::RsaSha256 => (Pkcs1v15Sign::new::<Sha256>(), Sha256::digest(base_string.as_bytes()).to_vec()),
                _ => (Pkcs1v15Sign::new::<Sha512>(), Sha512::digest(base_string.as_bytes()).to_vec()),
            };
            let signature = key.sign(padding, &digest).map_err(|err| invalid("oauth1", format!("RSA signing failed: {err}")))?;
            b64std(&signature)
        }
    };
    oauth.push(("oauth_signature".to_string(), signature));

    if auth.add_params_to_header {
        let mut fields = Vec::new();
        if !auth.realm.is_empty() {
            fields.push(format!("realm=\"{}\"", auth.realm.replace('"', "\\\"")));
        }
        for (key, value) in &oauth {
            fields.push(format!("{}=\"{}\"", pct(key), pct(value)));
        }
        Ok(OAuth1Output { header: Some(format!("OAuth {}", fields.join(", "))), query: Vec::new() })
    } else {
        Ok(OAuth1Output { header: None, query: oauth.into_iter().map(|(key, value)| (pct(&key), pct(&value))).collect() })
    }
}

fn is_form_body(auth: &OAuth1Auth, body: &[u8]) -> bool {
    if body.is_empty() {
        return false;
    }
    // The model's OAuth config does not carry a body content type; include pairs only when the
    // body looks form-encoded. Raw JSON/XML must not be parsed as OAuth parameters.
    let _ = auth;
    std::str::from_utf8(body).is_ok_and(|text| text.contains('=') && !text.trim_start().starts_with(['{', '[', '<']))
}

fn sha1_digest(body: &[u8]) -> Vec<u8> {
    Sha1::digest(body).to_vec()
}

fn rsa_key(auth: &OAuth1Auth) -> Result<RsaPrivateKey> {
    let pem = if auth.consumer_secret.contains("-----BEGIN") { &auth.consumer_secret } else { &auth.token_secret };
    if !pem.contains("-----BEGIN") {
        return Err(AuthError::Invalid { kind: "oauth1", reason: "RSA signing needs a PEM private key in consumer secret".into() });
    }
    RsaPrivateKey::from_pkcs8_pem(pem.trim())
        .or_else(|_| RsaPrivateKey::from_pkcs1_pem(pem.trim()))
        .map_err(|err| invalid("oauth1", format!("RSA private key: {err}")))
}

fn signature_name(method: OAuth1SignatureMethod) -> &'static str {
    match method {
        OAuth1SignatureMethod::HmacSha1 => "HMAC-SHA1",
        OAuth1SignatureMethod::HmacSha256 => "HMAC-SHA256",
        OAuth1SignatureMethod::HmacSha512 => "HMAC-SHA512",
        OAuth1SignatureMethod::RsaSha1 => "RSA-SHA1",
        OAuth1SignatureMethod::RsaSha256 => "RSA-SHA256",
        OAuth1SignatureMethod::RsaSha512 => "RSA-SHA512",
        OAuth1SignatureMethod::Plaintext => "PLAINTEXT",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_5849_hmac_sha1_example_matches() {
        let auth = OAuth1Auth {
            add_params_to_header: true,
            signature_method: OAuth1SignatureMethod::HmacSha1,
            consumer_key: "9djdj82h48djs9d2".into(),
            consumer_secret: "j49sk3j29djd".into(),
            token: "kkk9d7dh3k39sjv7".into(),
            token_secret: "dh893hdasih9".into(),
            ..OAuth1Auth::default()
        };
        let output = sign(&auth, "POST", "http://example.com/request?b5=%3D%253D&a3=a&c%40=&a2=r%20b", b"c2&a3=2+q", 137131201, "7d8f3e4a")
            .expect("signature");
        let header = output.header.expect("header");
        assert!(header.contains("oauth_signature=\"r6%2FTJjbCOr97%2F%2BUU0NsvSne7s5g%3D\""), "{header}");
        assert!(header.contains("oauth_signature_method=\"HMAC-SHA1\""));
    }

    #[test]
    fn plaintext_uses_both_secrets_and_query_placement_returns_pairs() {
        let auth = OAuth1Auth {
            signature_method: OAuth1SignatureMethod::Plaintext,
            consumer_key: "key".into(),
            consumer_secret: "cs".into(),
            token_secret: "ts".into(),
            ..OAuth1Auth::default()
        };
        let output = sign(&auth, "GET", "https://example.test/a", b"", 1, "n").expect("signature");
        assert!(output.header.is_none());
        assert!(output.query.iter().any(|(key, value)| key == "oauth_signature" && value == "cs%26ts"));
    }
}
