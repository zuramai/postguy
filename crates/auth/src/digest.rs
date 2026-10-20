//! HTTP Digest Access Authentication (RFC 7616/2617): parse a `WWW-Authenticate: Digest`
//! challenge and produce the `Authorization` answer. Supports MD5, MD5-sess, SHA-256,
//! SHA-256-sess and SHA-512-256 with `qop=auth` or `qop=auth-int`.

use std::collections::BTreeMap;

use postguy_model::auth::{DigestAlgorithm, DigestAuth};

use crate::error::{AuthError, Result, invalid, missing};
use crate::util::{md5_hex, sha256_hex, sha512_256_hex};

/// The parsed challenge parameters (unquoted values).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub realm: String,
    pub nonce: String,
    pub opaque: String,
    pub algorithm: String,
    pub qop: Vec<String>,
    pub userhash: bool,
    pub charset: String,
    pub stale: bool,
}

/// Parse the first `Digest` challenge from a `WWW-Authenticate` value. The value may contain
/// several schemes; parameters are comma-separated with quoted strings and escapes.
pub fn parse_challenge(value: &str) -> Result<Challenge> {
    let lower = value.to_ascii_lowercase();
    let mut search = 0;
    let mut start = None;
    while let Some(index) = lower.get(search..).and_then(|tail| tail.find("digest")).map(|index| search + index) {
        if index == 0 || value.as_bytes().get(index.wrapping_sub(1)).is_none_or(|byte| byte.is_ascii_whitespace() || *byte == b',') {
            start = Some(index);
            break;
        }
        search = index + 6;
    }
    let start = start.ok_or_else(|| invalid("digest", "no Digest challenge in WWW-Authenticate"))?;
    let rest = value.get(start + 6..).unwrap_or_default().trim_start();
    let params = parse_params(rest)?;
    let nonce = params.get("nonce").cloned().unwrap_or_default();
    if nonce.is_empty() {
        return Err(missing("digest", "nonce"));
    }
    Ok(Challenge {
        realm: params.get("realm").cloned().unwrap_or_default(),
        nonce,
        opaque: params.get("opaque").cloned().unwrap_or_default(),
        algorithm: params.get("algorithm").cloned().unwrap_or_else(|| "MD5".into()),
        qop: params
            .get("qop")
            .map(|value| value.split(',').map(|item| item.trim().to_ascii_lowercase()).filter(|item| !item.is_empty()).collect())
            .unwrap_or_default(),
        userhash: params.get("userhash").is_some_and(|value| value.eq_ignore_ascii_case("true")),
        charset: params.get("charset").cloned().unwrap_or_default(),
        stale: params.get("stale").is_some_and(|value| value.eq_ignore_ascii_case("true")),
    })
}

fn parse_params(input: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && (bytes[index] == b',' || bytes[index].is_ascii_whitespace()) {
            index += 1;
        }
        let start = index;
        while index < bytes.len() && bytes[index] != b'=' && bytes[index] != b',' {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'=' {
            break;
        }
        let name = input.get(start..index).unwrap_or_default().trim().to_ascii_lowercase();
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let value = if bytes.get(index) == Some(&b'"') {
            index += 1;
            let mut value = String::new();
            while index < bytes.len() {
                match bytes[index] {
                    b'\\' if index + 1 < bytes.len() => {
                        index += 1;
                        value.push(bytes[index] as char);
                        index += 1;
                    }
                    b'"' => {
                        index += 1;
                        break;
                    }
                    byte => {
                        value.push(byte as char);
                        index += 1;
                    }
                }
            }
            value
        } else {
            let start = index;
            while index < bytes.len() && bytes[index] != b',' {
                index += 1;
            }
            input.get(start..index).unwrap_or_default().trim().to_string()
        };
        out.insert(name, value);
    }
    Ok(out)
}

/// A digest answer, including the header value and the next nonce count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub authorization: String,
    pub nonce_count: String,
}

/// Build the Authorization answer. `body` is only read for `auth-int`.
pub fn answer(auth: &DigestAuth, challenge: &Challenge, method: &str, uri: &str, body: &[u8], cnonce: &str, nonce_count: u32) -> Result<Answer> {
    if auth.username.is_empty() {
        return Err(missing("digest", "username"));
    }
    let algorithm = algorithm(&challenge.algorithm).ok_or_else(|| AuthError::Unsupported {
        kind: "digest",
        reason: format!("unsupported algorithm `{}` (use MD5, MD5-sess, SHA-256, SHA-256-sess or SHA-512-256)", challenge.algorithm),
    })?;
    let qop = if challenge.qop.iter().any(|value| value == "auth") {
        Some("auth")
    } else if challenge.qop.iter().any(|value| value == "auth-int") {
        Some("auth-int")
    } else {
        None
    };
    let hash = |value: &str| match algorithm {
        DigestAlgorithm::Md5 | DigestAlgorithm::Md5Sess => md5_hex(value.as_bytes()),
        DigestAlgorithm::Sha256 | DigestAlgorithm::Sha256Sess => sha256_hex(value.as_bytes()),
        DigestAlgorithm::Sha512256 => sha512_256_hex(value.as_bytes()),
    };
    let mut ha1 = hash(&format!("{}:{}:{}", auth.username, challenge.realm, auth.password));
    if matches!(algorithm, DigestAlgorithm::Md5Sess | DigestAlgorithm::Sha256Sess) {
        ha1 = hash(&format!("{ha1}:{}:{cnonce}", challenge.nonce));
    }
    let ha2 = match qop {
        Some("auth-int") => hash(&format!("{method}:{uri}:{}", hash_bytes(algorithm, body))),
        _ => hash(&format!("{method}:{uri}")),
    };
    let nc = format!("{nonce_count:08x}");
    let response = match qop {
        Some(qop) => hash(&format!("{ha1}:{}:{nc}:{cnonce}:{qop}:{ha2}", challenge.nonce)),
        None => hash(&format!("{ha1}:{}:{ha2}", challenge.nonce)),
    };
    let username = if challenge.userhash {
        // RFC 7616 userhash: H(username:realm), hex.
        hash(&format!("{}:{}", auth.username, challenge.realm))
    } else {
        auth.username.clone()
    };
    let mut fields = vec![
        format!("username=\"{}\"", quote(&username)),
        format!("realm=\"{}\"", quote(&challenge.realm)),
        format!("uri=\"{}\"", quote(uri)),
        format!("algorithm={}", challenge.algorithm),
        format!("nonce=\"{}\"", quote(&challenge.nonce)),
    ];
    if let Some(qop) = qop {
        fields.push(format!("nc={nc}"));
        fields.push(format!("cnonce=\"{}\"", quote(cnonce)));
        fields.push(format!("qop={qop}"));
    }
    fields.push(format!("response=\"{response}\""));
    if !challenge.opaque.is_empty() {
        fields.push(format!("opaque=\"{}\"", quote(&challenge.opaque)));
    }
    if challenge.userhash {
        fields.push("userhash=true".into());
    }
    Ok(Answer { authorization: format!("Digest {}", fields.join(", ")), nonce_count: nc })
}

fn hash_bytes(algorithm: DigestAlgorithm, body: &[u8]) -> String {
    match algorithm {
        DigestAlgorithm::Md5 | DigestAlgorithm::Md5Sess => md5_hex(body),
        DigestAlgorithm::Sha256 | DigestAlgorithm::Sha256Sess => sha256_hex(body),
        DigestAlgorithm::Sha512256 => sha512_256_hex(body),
    }
}

fn algorithm(value: &str) -> Option<DigestAlgorithm> {
    Some(match value.to_ascii_lowercase().as_str() {
        "md5" => DigestAlgorithm::Md5,
        "md5-sess" => DigestAlgorithm::Md5Sess,
        "sha-256" | "sha256" => DigestAlgorithm::Sha256,
        "sha-256-sess" | "sha256-sess" => DigestAlgorithm::Sha256Sess,
        "sha-512-256" => DigestAlgorithm::Sha512256,
        _ => return None,
    })
}

fn quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> DigestAuth {
        DigestAuth {
            username: "Mufasa".into(),
            password: "Circle of Life".into(),
            realm: "http-auth@example.org".into(),
            nonce: "7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v".into(),
            opaque: "FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS".into(),
            ..DigestAuth::default()
        }
    }

    fn challenge(algorithm: &str) -> Challenge {
        parse_challenge(&format!(
            "Digest realm=\"http-auth@example.org\", qop=\"auth, auth-int\", algorithm={algorithm}, nonce=\"7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v\", opaque=\"FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS\""
        ))
        .expect("challenge")
    }

    #[test]
    fn rfc_7616_md5_example_matches() {
        let answer =
            answer(&auth(), &challenge("MD5"), "GET", "/dir/index.html", b"", "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ", 1).expect("answer");
        assert!(answer.authorization.contains("response=\"8ca523f5e9506fed4657c9700eebdbec\""), "{}", answer.authorization);
        assert_eq!(answer.nonce_count, "00000001");
    }

    #[test]
    fn rfc_7616_sha256_example_matches() {
        let answer =
            answer(&auth(), &challenge("SHA-256"), "GET", "/dir/index.html", b"", "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ", 1).expect("answer");
        assert!(
            answer.authorization.contains("response=\"753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1\""),
            "{}",
            answer.authorization
        );
    }

    #[test]
    fn malformed_and_unknown_challenges_are_errors() {
        assert!(parse_challenge("Bearer realm=\"x\"").is_err());
        assert!(parse_challenge("Digest realm=\"x\"").is_err(), "missing nonce");
        let error = answer(&auth(), &challenge("SHA-1"), "GET", "/", b"", "abc", 1).expect_err("unsupported");
        assert!(error.to_string().contains("unsupported algorithm"), "{error}");
    }
}
