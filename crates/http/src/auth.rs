//! Auth application (PROMPT.md 5.3.2 Authorization tab, v1 P0 set): Basic, Bearer, API key.
//! Values arrive ALREADY resolved by the engine (variables expanded, secrets fetched); this
//! module only places them on the request and never logs or returns them.
//!
//! `Inherit` and `NoAuth` do nothing (the engine resolves inheritance before calling); an auth
//! kind outside the v1 set is an actionable error rather than a silently unauthenticated send.

use postguy_model::AuthConfig;
use postguy_model::auth::{AuthLocation, DigestAlgorithm};
use postguy_urlkit::{Param, Url, params_of_query, query_of_params};

use crate::client::RequestSpec;
use crate::error::{HttpError, Result};
use crate::proxy::base64_encode;

/// Where a header written by auth came from, so the UI can mark the row (auto-generated and
/// read-only) and the importer/report can explain it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthPlacement {
    Header,
    Query,
}

/// Apply `config`. The spec's header name match is case-insensitive; an existing hand-written
/// header of the same name is replaced (the UI's auto header wins on send).
pub fn apply_auth(config: &AuthConfig, spec: &mut RequestSpec) -> Result<AuthPlacement> {
    match config {
        AuthConfig::Inherit | AuthConfig::NoAuth | AuthConfig::Unknown { .. } => Ok(AuthPlacement::Header),
        AuthConfig::Basic(auth) => {
            let value = base64_encode(format!("{}:{}", auth.username, auth.password).as_bytes());
            set_header(spec, "Authorization", format!("Basic {value}"));
            Ok(AuthPlacement::Header)
        }
        AuthConfig::Bearer(auth) => {
            if auth.token.is_empty() {
                return Err(HttpError::Auth { kind: "bearer".into(), reason: "the token is empty".into() });
            }
            let prefix = auth.prefix.as_deref().unwrap_or("Bearer");
            let value = if prefix.trim().is_empty() { auth.token.clone() } else { format!("{prefix} {}", auth.token) };
            set_header(spec, "Authorization", value);
            Ok(AuthPlacement::Header)
        }
        AuthConfig::ApiKey(auth) => {
            if auth.key.trim().is_empty() {
                return Err(HttpError::Auth { kind: "apikey".into(), reason: "the key name is empty".into() });
            }
            match auth.location {
                AuthLocation::Header => {
                    set_header(spec, auth.key.trim(), auth.value.clone());
                    Ok(AuthPlacement::Header)
                }
                AuthLocation::Query => {
                    let mut url = Url::parse(&spec.url);
                    let mut params = url.query.as_ref().map(|query| params_of_query(&query.0)).unwrap_or_default();
                    if let Some(existing) = params.iter_mut().find(|param| param.key == auth.key.trim()) {
                        existing.value = auth.value.clone();
                    } else {
                        params.push(Param { key: auth.key.trim().to_string(), value: auth.value.clone() });
                    }
                    url.query = query_of_params(&params);
                    spec.url = url.build();
                    Ok(AuthPlacement::Query)
                }
            }
        }
        AuthConfig::Digest(auth) => {
            if auth.nonce.trim().is_empty() {
                return Err(HttpError::Auth { kind: "digest".into(), reason: "waiting for the server's Digest challenge".into() });
            }
            let uri = digest_uri(&spec.url);
            let cnonce = if auth.client_nonce.is_empty() { postguy_auth::random_nonce() } else { auth.client_nonce.clone() };
            let count = parse_nonce_count(&auth.nonce_count);
            let challenge = postguy_auth::digest::Challenge {
                realm: auth.realm.clone(),
                nonce: auth.nonce.clone(),
                opaque: auth.opaque.clone(),
                algorithm: digest_algorithm_name(auth.algorithm).into(),
                qop: auth.qop.split(',').map(|value| value.trim().to_ascii_lowercase()).filter(|value| !value.is_empty()).collect(),
                userhash: false,
                charset: String::new(),
                stale: false,
            };
            let answer =
                postguy_auth::digest::answer(auth, &challenge, spec.method.as_str(), &uri, &spec.body, &cnonce, count).map_err(auth_error)?;
            set_header(spec, "Authorization", answer.authorization);
            Ok(AuthPlacement::Header)
        }
        AuthConfig::Jwt(auth) => {
            let token = postguy_auth::jwt::encode(auth, postguy_auth::now_secs()).map_err(auth_error)?;
            match auth.location {
                AuthLocation::Header => {
                    let prefix = auth.prefix.as_deref().unwrap_or("Bearer");
                    let value = if prefix.trim().is_empty() { token } else { format!("{prefix} {token}") };
                    set_header(spec, "Authorization", value);
                    Ok(AuthPlacement::Header)
                }
                AuthLocation::Query => {
                    append_query(spec, "token", &token);
                    Ok(AuthPlacement::Query)
                }
            }
        }
        AuthConfig::OAuth1(auth) => {
            let nonce = if auth.nonce.trim().is_empty() { postguy_auth::random_nonce() } else { auth.nonce.clone() };
            let output = postguy_auth::oauth1::sign(auth, spec.method.as_str(), &spec.url, &spec.body, postguy_auth::now_secs(), &nonce)
                .map_err(auth_error)?;
            if let Some(header) = output.header {
                set_header(spec, "Authorization", header);
                Ok(AuthPlacement::Header)
            } else {
                for (key, value) in output.query {
                    append_encoded_query(spec, &key, &value);
                }
                Ok(AuthPlacement::Query)
            }
        }
        AuthConfig::AwsV4(auth) => {
            let output = postguy_auth::aws::sign(auth, spec.method.as_str(), &spec.url, &spec.headers, &spec.body, postguy_auth::now_secs())
                .map_err(auth_error)?;
            for (name, value) in output.headers {
                set_header(spec, &name, value);
            }
            if let Some(header) = output.header {
                set_header(spec, "Authorization", header);
                Ok(AuthPlacement::Header)
            } else {
                replace_encoded_query(spec, &output.query);
                Ok(AuthPlacement::Query)
            }
        }
        AuthConfig::OAuth2(auth) => {
            if auth.access_token.is_empty() {
                return Err(HttpError::Auth { kind: "oauth2".into(), reason: "no access token; run the authorization flow first".into() });
            }
            if auth.access_token_expires_at_ms > 0 && auth.access_token_expires_at_ms <= postguy_util::time::now_unix_ms() {
                return Err(HttpError::Auth { kind: "oauth2".into(), reason: "the access token has expired; refresh it first".into() });
            }
            match auth.location {
                postguy_model::auth::OAuth2Location::Header => {
                    let prefix = if auth.header_prefix.is_empty() { "Bearer" } else { auth.header_prefix.as_str() };
                    let value = if prefix.trim().is_empty() { auth.access_token.clone() } else { format!("{prefix} {}", auth.access_token) };
                    set_header(spec, "Authorization", value);
                    Ok(AuthPlacement::Header)
                }
                postguy_model::auth::OAuth2Location::Url => {
                    let name = if auth.token_name.is_empty() { "access_token" } else { auth.token_name.as_str() };
                    append_query(spec, name, &auth.access_token);
                    Ok(AuthPlacement::Query)
                }
            }
        }
        AuthConfig::Hawk(auth) => {
            let value =
                postguy_auth::hawk::authorization(auth, spec.method.as_str(), &spec.url, &spec.body, postguy_auth::now_secs()).map_err(auth_error)?;
            set_header(spec, "Authorization", value);
            Ok(AuthPlacement::Header)
        }
        other => Err(HttpError::Auth {
            kind: other.kind(),
            reason: "this auth type is not supported yet (v1 sends Basic, Bearer, API key, Digest, JWT, OAuth 1.0, AWS v4 and Hawk)".into(),
        }),
    }
}

fn auth_error(err: postguy_auth::AuthError) -> HttpError {
    HttpError::Auth { kind: "auth".into(), reason: err.to_string() }
}

fn digest_uri(url: &str) -> String {
    let parsed = Url::parse(url);
    match parsed.query.as_ref() {
        Some(query) if !query.0.is_empty() => format!("{}?{}", parsed.path, query.0),
        _ => parsed.path,
    }
}

fn digest_algorithm_name(algorithm: DigestAlgorithm) -> &'static str {
    match algorithm {
        DigestAlgorithm::Md5 => "MD5",
        DigestAlgorithm::Md5Sess => "MD5-sess",
        DigestAlgorithm::Sha256 => "SHA-256",
        DigestAlgorithm::Sha256Sess => "SHA-256-sess",
        DigestAlgorithm::Sha512256 => "SHA-512-256",
    }
}

fn parse_nonce_count(value: &str) -> u32 {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return 1;
    }
    u32::from_str_radix(trimmed, 16).or_else(|_| trimmed.parse()).unwrap_or(1)
}

fn append_encoded_query(spec: &mut RequestSpec, key: &str, value: &str) {
    if key.is_empty() {
        return;
    }
    if spec.url.contains('?') {
        if !spec.url.ends_with('?') && !spec.url.ends_with('&') {
            spec.url.push('&');
        }
    } else {
        spec.url.push('?');
    }
    spec.url.push_str(key);
    spec.url.push('=');
    spec.url.push_str(value);
}

fn replace_encoded_query(spec: &mut RequestSpec, pairs: &[(String, String)]) {
    let base = spec.url.split(['?', '#']).next().unwrap_or_default().to_string();
    let query = pairs.iter().map(|(key, value)| format!("{key}={value}")).collect::<Vec<_>>().join("&");
    spec.url = if query.is_empty() { base } else { format!("{base}?{query}") };
}

fn append_query(spec: &mut RequestSpec, key: &str, value: &str) {
    let mut url = Url::parse(&spec.url);
    let mut params = url.query.as_ref().map(|query| params_of_query(&query.0)).unwrap_or_default();
    if let Some(existing) = params.iter_mut().find(|param| param.key == key) {
        existing.value = postguy_urlkit::decode_percent(value);
    } else {
        params.push(Param { key: key.to_string(), value: postguy_urlkit::decode_percent(value) });
    }
    url.query = query_of_params(&params);
    spec.url = url.build();
}

fn set_header(spec: &mut RequestSpec, name: &str, value: String) {
    if let Some(existing) = spec.headers.iter_mut().find(|(header, _)| header.eq_ignore_ascii_case(name)) {
        existing.1 = value;
    } else {
        spec.headers.push((name.to_string(), value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postguy_model::auth::{ApiKeyAuth, BasicAuth, BearerAuth};

    fn spec_with(url: &str) -> RequestSpec {
        RequestSpec::get(url)
    }

    fn basic(username: &str, password: &str) -> AuthConfig {
        AuthConfig::Basic(Box::new(BasicAuth { username: username.into(), password: password.into(), ..BasicAuth::default() }))
    }

    #[test]
    fn basic_writes_the_rfc_7617_header_and_replaces_a_manual_one() {
        let mut spec = spec_with("http://example.com/");
        spec.headers.push(("Authorization".into(), "stale".into()));
        assert_eq!(apply_auth(&basic("user", "p@ss"), &mut spec).expect("auth"), AuthPlacement::Header);
        let header = spec.headers.iter().find(|(name, _)| name == "Authorization").expect("header");
        assert_eq!(header.1, "Basic dXNlcjpwQHNz");
        assert_eq!(spec.headers.iter().filter(|(name, _)| name == "Authorization").count(), 1);
    }

    #[test]
    fn bearer_default_prefix_is_bearer_and_empty_prefix_uses_the_token_alone() {
        let mut spec = spec_with("http://example.com/");
        apply_auth(&AuthConfig::Bearer(Box::new(BearerAuth { token: "abc".into(), prefix: None, ..BearerAuth::default() })), &mut spec)
            .expect("auth");
        assert_eq!(spec.headers.iter().find(|(name, _)| name == "Authorization").map(|(_, value)| value.as_str()), Some("Bearer abc"));
        let mut spec = spec_with("http://example.com/");
        apply_auth(
            &AuthConfig::Bearer(Box::new(BearerAuth { token: "abc".into(), prefix: Some(String::new()), ..BearerAuth::default() })),
            &mut spec,
        )
        .expect("auth");
        assert_eq!(spec.headers.iter().find(|(name, _)| name == "Authorization").map(|(_, value)| value.as_str()), Some("abc"));
    }

    #[test]
    fn api_key_goes_to_a_header_or_the_query_with_replacement() {
        let mut spec = spec_with("http://example.com/a?x=1");
        apply_auth(
            &AuthConfig::ApiKey(Box::new(ApiKeyAuth {
                key: "X-Key".into(),
                value: "k".into(),
                location: AuthLocation::Header,
                ..ApiKeyAuth::default()
            })),
            &mut spec,
        )
        .expect("auth");
        assert_eq!(spec.headers.iter().find(|(name, _)| name == "X-Key").map(|(_, value)| value.as_str()), Some("k"));

        let mut spec = spec_with("http://example.com/a?x=1");
        apply_auth(
            &AuthConfig::ApiKey(Box::new(ApiKeyAuth {
                key: "api_key".into(),
                value: "k".into(),
                location: AuthLocation::Query,
                ..ApiKeyAuth::default()
            })),
            &mut spec,
        )
        .expect("auth");
        assert_eq!(spec.url, "http://example.com/a?x=1&api_key=k");
        // A second application replaces the value instead of duplicating the key.
        apply_auth(
            &AuthConfig::ApiKey(Box::new(ApiKeyAuth {
                key: "api_key".into(),
                value: "k2".into(),
                location: AuthLocation::Query,
                ..ApiKeyAuth::default()
            })),
            &mut spec,
        )
        .expect("auth");
        assert_eq!(spec.url, "http://example.com/a?x=1&api_key=k2");
    }

    #[test]
    fn jwt_oauth1_aws_and_hawk_place_their_headers() {
        let mut spec = spec_with("http://example.com/");
        apply_auth(
            &AuthConfig::Jwt(Box::new(postguy_model::auth::JwtAuth {
                secret: "secret".into(),
                payload: r#"{"sub":"ada"}"#.into(),
                ..postguy_model::auth::JwtAuth::default()
            })),
            &mut spec,
        )
        .expect("jwt");
        let header = spec.headers.iter().find(|(name, _)| name == "Authorization").map(|(_, value)| value.as_str()).unwrap_or_default();
        assert!(header.starts_with("Bearer "), "{header}");

        let mut spec = spec_with("http://example.com/");
        apply_auth(
            &AuthConfig::OAuth1(Box::new(postguy_model::auth::OAuth1Auth {
                add_params_to_header: true,
                consumer_key: "ck".into(),
                consumer_secret: "cs".into(),
                ..postguy_model::auth::OAuth1Auth::default()
            })),
            &mut spec,
        )
        .expect("oauth1");
        assert!(spec.headers.iter().any(|(name, value)| name == "Authorization" && value.starts_with("OAuth ")));
        assert!(spec.headers.iter().any(|(name, _)| name == "Authorization"));

        let mut spec = spec_with("https://example.amazonaws.com/");
        apply_auth(
            &AuthConfig::AwsV4(Box::new(postguy_model::auth::AwsV4Auth {
                access_key: "AKIDEXAMPLE".into(),
                secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
                region: "us-east-1".into(),
                service: "service".into(),
                ..postguy_model::auth::AwsV4Auth::default()
            })),
            &mut spec,
        )
        .expect("aws");
        assert!(spec.headers.iter().any(|(name, _)| name == "x-amz-date"));
        assert!(spec.headers.iter().any(|(name, value)| name == "Authorization" && value.starts_with("AWS4-HMAC-SHA256 ")));

        let mut spec = spec_with("http://example.com/");
        apply_auth(
            &AuthConfig::Hawk(Box::new(postguy_model::auth::HawkAuth {
                auth_id: "id".into(),
                auth_key: "key".into(),
                ..postguy_model::auth::HawkAuth::default()
            })),
            &mut spec,
        )
        .expect("hawk");
        assert!(spec.headers.iter().any(|(name, value)| name == "Authorization" && value.starts_with("Hawk ")));
    }

    #[test]
    fn oauth2_access_token_goes_to_the_header_or_url_and_rejects_expiry() {
        let mut auth = postguy_model::auth::OAuth2Auth { access_token: "abc".into(), ..postguy_model::auth::OAuth2Auth::default() };
        let mut spec = spec_with("http://example.com/");
        apply_auth(&AuthConfig::OAuth2(Box::new(auth.clone())), &mut spec).expect("oauth2");
        assert!(spec.headers.iter().any(|(name, value)| name == "Authorization" && value == "Bearer abc"));

        auth.location = postguy_model::auth::OAuth2Location::Url;
        auth.token_name = "access".into();
        let mut spec = spec_with("http://example.com/a?x=1");
        apply_auth(&AuthConfig::OAuth2(Box::new(auth.clone())), &mut spec).expect("query");
        assert_eq!(spec.url, "http://example.com/a?x=1&access=abc");

        auth.access_token_expires_at_ms = 1;
        let mut spec = spec_with("http://example.com/");
        let error = apply_auth(&AuthConfig::OAuth2(Box::new(auth)), &mut spec).expect_err("expired");
        assert!(error.to_string().contains("expired"), "{error}");
    }

    #[test]
    fn empty_credentials_and_unsupported_types_are_actionable_errors() {
        let mut spec = spec_with("http://example.com/");
        let digest = AuthConfig::Digest(Box::default());
        let error = apply_auth(&digest, &mut spec).expect_err("no error");
        assert!(error.to_string().contains("Digest challenge"), "digest without a challenge is deferred: {error}");
        let error = apply_auth(&AuthConfig::Bearer(Box::default()), &mut spec).expect_err("no error");
        assert!(error.to_string().contains("token is empty"), "{error}");
        let error = apply_auth(&AuthConfig::ApiKey(Box::default()), &mut spec).expect_err("no error");
        assert!(error.to_string().contains("key name is empty"), "{error}");
    }

    #[test]
    fn inherit_and_no_auth_touch_nothing() {
        let mut spec = spec_with("http://example.com/");
        apply_auth(&AuthConfig::NoAuth, &mut spec).expect("no auth");
        apply_auth(&AuthConfig::Inherit, &mut spec).expect("inherit");
        assert!(spec.headers.is_empty());
        assert_eq!(spec.url, "http://example.com/");
    }
}
