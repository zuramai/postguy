//! JWT bearer tokens: HS256/384/512, RS256/384/512, ES256/384/512 and
//! PS256/384/512. The claims JSON is user text; `iat` is added when absent. Private keys are
//! PEM (PKCS#8 or PKCS#1 for RSA); ECDSA keys are PKCS#8. Errors never echo key material.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use postguy_model::auth::{JwtAlgorithm, JwtAuth};
use rsa::pkcs1::DecodeRsaPrivateKey as _;
use rsa::pkcs8::DecodePrivateKey as _;
use rsa::pss::SigningKey as PssSigningKey;
use rsa::signature::{RandomizedSigner as _, SignatureEncoding as _};
use rsa::{Pkcs1v15Sign, RsaPrivateKey};
use sha2::{Digest as _, Sha256, Sha384, Sha512};

use crate::error::{Result, invalid, missing};
use crate::util::{MacAlgorithm, b64url, hmac};

/// Encode a JWT for the given auth config at `now` (unix seconds).
pub fn encode(auth: &JwtAuth, now: u64) -> Result<String> {
    let algorithm = auth.algorithm;
    let mut header = serde_json::Map::new();
    header.insert("typ".into(), serde_json::Value::String("JWT".into()));
    header.insert("alg".into(), serde_json::Value::String(algorithm_name(algorithm).into()));
    for extra in &auth.headers {
        if !extra.name.trim().is_empty() {
            header.insert(extra.name.clone(), serde_json::Value::String(extra.value.clone()));
        }
    }
    let mut payload: serde_json::Value = if auth.payload.trim().is_empty() {
        serde_json::Value::Object(Default::default())
    } else {
        serde_json::from_str(&auth.payload).map_err(|err| invalid("jwt", format!("claims are not JSON: {err}")))?
    };
    let Some(object) = payload.as_object_mut() else {
        return Err(invalid("jwt", "claims must be a JSON object"));
    };
    object.entry("iat").or_insert_with(|| serde_json::json!(now));
    let signing_input = format!(
        "{}.{}",
        b64url(serde_json::to_string(&header).map_err(|err| invalid("jwt", err.to_string()))?.as_bytes()),
        b64url(payload.to_string().as_bytes())
    );
    let signature = sign(auth, algorithm, signing_input.as_bytes())?;
    Ok(format!("{signing_input}.{signature}"))
}

fn sign(auth: &JwtAuth, algorithm: JwtAlgorithm, input: &[u8]) -> Result<String> {
    match algorithm {
        JwtAlgorithm::Hs256 | JwtAlgorithm::Hs384 | JwtAlgorithm::Hs512 => {
            if auth.secret.is_empty() {
                return Err(missing("jwt", "secret"));
            }
            let key = if auth.secret_is_base64 {
                STANDARD
                    .decode(auth.secret.trim())
                    .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(auth.secret.trim()))
                    .map_err(|err| invalid("jwt", format!("secret is not base64: {err}")))?
            } else {
                auth.secret.as_bytes().to_vec()
            };
            let mac = match algorithm {
                JwtAlgorithm::Hs256 => MacAlgorithm::Sha256,
                JwtAlgorithm::Hs384 => MacAlgorithm::Sha384,
                _ => MacAlgorithm::Sha512,
            };
            Ok(b64url(&hmac(mac, &key, input)?))
        }
        JwtAlgorithm::Rs256 | JwtAlgorithm::Rs384 | JwtAlgorithm::Rs512 => {
            let key = rsa_key(auth)?;
            let (padding, digest) = match algorithm {
                JwtAlgorithm::Rs256 => (Pkcs1v15Sign::new::<Sha256>(), Sha256::digest(input).to_vec()),
                JwtAlgorithm::Rs384 => (Pkcs1v15Sign::new::<Sha384>(), Sha384::digest(input).to_vec()),
                _ => (Pkcs1v15Sign::new::<Sha512>(), Sha512::digest(input).to_vec()),
            };
            let signature = key.sign(padding, &digest).map_err(|err| invalid("jwt", format!("RSA signing failed: {err}")))?;
            Ok(b64url(&signature))
        }
        JwtAlgorithm::Ps256 | JwtAlgorithm::Ps384 | JwtAlgorithm::Ps512 => {
            let key = rsa_key(auth)?;
            let mut rng = rsa::rand_core::OsRng;
            let signature = match algorithm {
                JwtAlgorithm::Ps256 => PssSigningKey::<Sha256>::new(key).sign_with_rng(&mut rng, input),
                JwtAlgorithm::Ps384 => PssSigningKey::<Sha384>::new(key).sign_with_rng(&mut rng, input),
                _ => PssSigningKey::<Sha512>::new(key).sign_with_rng(&mut rng, input),
            };
            Ok(b64url(&signature.to_bytes()))
        }
        JwtAlgorithm::Es256 | JwtAlgorithm::Es384 | JwtAlgorithm::Es512 => {
            if auth.private_key.trim().is_empty() {
                return Err(missing("jwt", "private key"));
            }
            let pem = auth.private_key.trim();
            let signature = match algorithm {
                JwtAlgorithm::Es256 => {
                    let key = p256::SecretKey::from_pkcs8_pem(pem).map_err(|err| invalid("jwt", format!("P-256 key: {err}")))?;
                    let signer = p256::ecdsa::SigningKey::from(key);
                    let signature = <p256::ecdsa::SigningKey as signature::Signer<p256::ecdsa::Signature>>::sign(&signer, input);
                    signature.to_bytes().to_vec()
                }
                JwtAlgorithm::Es384 => {
                    let key = p384::SecretKey::from_pkcs8_pem(pem).map_err(|err| invalid("jwt", format!("P-384 key: {err}")))?;
                    let signer = p384::ecdsa::SigningKey::from(key);
                    let signature = <p384::ecdsa::SigningKey as signature::Signer<p384::ecdsa::Signature>>::sign(&signer, input);
                    signature.to_bytes().to_vec()
                }
                _ => {
                    let key = p521::SecretKey::from_pkcs8_pem(pem).map_err(|err| invalid("jwt", format!("P-521 key: {err}")))?;
                    let signer =
                        p521::ecdsa::SigningKey::from_slice(&key.to_bytes()).map_err(|err| invalid("jwt", format!("P-521 signing key: {err}")))?;
                    let signature = <p521::ecdsa::SigningKey as signature::Signer<p521::ecdsa::Signature>>::sign(&signer, input);
                    signature.to_bytes().to_vec()
                }
            };
            Ok(b64url(&signature))
        }
    }
}

fn rsa_key(auth: &JwtAuth) -> Result<RsaPrivateKey> {
    if auth.private_key.trim().is_empty() {
        return Err(missing("jwt", "private key"));
    }
    let pem = auth.private_key.trim();
    RsaPrivateKey::from_pkcs8_pem(pem)
        .or_else(|_| RsaPrivateKey::from_pkcs1_pem(pem))
        .map_err(|err| invalid("jwt", format!("RSA private key: {err}")))
}

pub fn algorithm_name(algorithm: JwtAlgorithm) -> &'static str {
    match algorithm {
        JwtAlgorithm::Hs256 => "HS256",
        JwtAlgorithm::Hs384 => "HS384",
        JwtAlgorithm::Hs512 => "HS512",
        JwtAlgorithm::Rs256 => "RS256",
        JwtAlgorithm::Rs384 => "RS384",
        JwtAlgorithm::Rs512 => "RS512",
        JwtAlgorithm::Es256 => "ES256",
        JwtAlgorithm::Es384 => "ES384",
        JwtAlgorithm::Es512 => "ES512",
        JwtAlgorithm::Ps256 => "PS256",
        JwtAlgorithm::Ps384 => "PS384",
        JwtAlgorithm::Ps512 => "PS512",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_KEY: &str = "AyM1SysPpbyDfgZld3umj1qzKObwVMkoqQ-EstJQLr_T-1qS0gZH75aKtMN3Yj0iPS4hcgUuTwjAzZr1Z9CAow";

    #[test]
    fn rfc_7515_hs256_mac_matches_the_appendix() {
        let input = b"eyJ0eXAiOiJKV1QiLA0KICJhbGciOiJIUzI1NiJ9.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ";
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(RFC_KEY).expect("key");
        let mac = hmac(MacAlgorithm::Sha256, &key, input).expect("mac");
        assert_eq!(b64url(&mac), "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    }

    #[test]
    fn compact_hs_token_has_three_parts_and_iat() {
        let auth = JwtAuth { algorithm: JwtAlgorithm::Hs256, secret: "secret".into(), payload: r#"{"sub":"ada"}"#.into(), ..JwtAuth::default() };
        let token = encode(&auth, 1_700_000_000).expect("token");
        assert_eq!(token.split('.').count(), 3);
        let payload = token.split('.').nth(1).expect("payload");
        let json: serde_json::Value =
            serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).expect("payload bytes")).expect("payload json");
        assert_eq!(json["iat"], 1_700_000_000);
    }

    #[test]
    fn rsa_and_ecdsa_tokens_verify_with_their_public_keys() {
        use rsa::pkcs8::{EncodePrivateKey as _, LineEnding};
        use rsa::signature::Verifier as _;
        use rsa::traits::PublicKeyParts as _;

        let mut rng = rsa::rand_core::OsRng;
        let rsa_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa key");
        let pem = rsa_key.to_pkcs8_pem(LineEnding::LF).expect("rsa pem").to_string();
        let public = rsa::RsaPublicKey::from(&rsa_key);
        let payload = r#"{"sub":"ada"}"#;
        for algorithm in [JwtAlgorithm::Rs256, JwtAlgorithm::Ps256] {
            let auth = JwtAuth { algorithm, private_key: pem.clone(), payload: payload.into(), ..JwtAuth::default() };
            let token = encode(&auth, 1).expect("rsa token");
            let mut parts = token.split('.');
            let header = parts.next().expect("header");
            let body = parts.next().expect("body");
            let signature = parts.next().expect("signature");
            let signing_input = format!("{header}.{body}");
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature).expect("sig bytes");
            match algorithm {
                JwtAlgorithm::Rs256 => {
                    let key = rsa::pkcs1v15::VerifyingKey::<Sha256>::new(public.clone());
                    let signature = rsa::pkcs1v15::Signature::try_from(raw.as_slice()).expect("signature");
                    key.verify(signing_input.as_bytes(), &signature).expect("RS256 verifies");
                }
                _ => {
                    let key = rsa::pss::VerifyingKey::<Sha256>::new(public.clone());
                    let signature = rsa::pss::Signature::try_from(raw.as_slice()).expect("signature");
                    key.verify(signing_input.as_bytes(), &signature).expect("PS256 verifies");
                }
            }
            assert_eq!(public.size(), 256);
        }

        let secret = p256::SecretKey::random(&mut rng);
        let pem = p256::pkcs8::EncodePrivateKey::to_pkcs8_pem(&secret, LineEnding::LF).expect("ec pem").to_string();
        let auth = JwtAuth { algorithm: JwtAlgorithm::Es256, private_key: pem, payload: payload.into(), ..JwtAuth::default() };
        let token = encode(&auth, 1).expect("es token");
        let (signing_input, signature) = token.rsplit_once('.').expect("token parts");
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature).expect("sig bytes");
        let signature = p256::ecdsa::Signature::from_slice(&raw).expect("ec signature");
        let signer = p256::ecdsa::SigningKey::from(secret.clone());
        let key = signer.verifying_key();
        key.verify(signing_input.as_bytes(), &signature).expect("ES256 verifies");
    }

    #[test]
    fn malformed_claims_and_missing_keys_are_actionable() {
        let auth = JwtAuth { payload: "[]".into(), secret: "x".into(), ..JwtAuth::default() };
        assert!(encode(&auth, 1).is_err());
        let auth = JwtAuth { private_key: "not a key".into(), algorithm: JwtAlgorithm::Rs256, ..JwtAuth::default() };
        let error = encode(&auth, 1).expect_err("bad key");
        assert!(error.to_string().contains("RSA private key"), "{error}");
        let auth = JwtAuth { algorithm: JwtAlgorithm::Es256, ..JwtAuth::default() };
        assert!(encode(&auth, 1).is_err(), "missing EC key");
    }
}
