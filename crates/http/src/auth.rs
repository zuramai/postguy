//! Auth application (PROMPT.md 5.3.2 Authorization tab, v1 P0 set): Basic, Bearer, API key.
//! Values arrive ALREADY resolved by the engine (variables expanded, secrets fetched); this
//! module only places them on the request and never logs or returns them.
//!
//! `Inherit` and `NoAuth` do nothing (the engine resolves inheritance before calling); an auth
//! kind outside the v1 set is an actionable error rather than a silently unauthenticated send.

use postguy_model::AuthConfig;
use postguy_model::auth::AuthLocation;
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
        other => {
            Err(HttpError::Auth { kind: other.kind(), reason: "this auth type is not supported yet (v1 sends Basic, Bearer and API key)".into() })
        }
    }
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
    fn empty_credentials_and_unsupported_types_are_actionable_errors() {
        let mut spec = spec_with("http://example.com/");
        let digest = AuthConfig::Digest(Box::default());
        let error = apply_auth(&digest, &mut spec).expect_err("no error");
        assert!(error.to_string().contains("not supported"), "digest is not in the v1 set: {error}");
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
