//! Auth configurations (PROMPT.md 4.5, Appendix E). Typed structs per auth type; every field
//! holds raw strings (`{{variables}}` unresolved — resolution is the send pipeline's job).
//! Unknown keys inside each config are preserved in `extras`; auth types an import could not
//! map land in [`AuthConfig::Unknown`] with everything they carried.

use crate::extras::Extras;
use serde::{Deserialize, Serialize};

/// Where a header-or-query auth puts its data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AuthLocation {
    #[default]
    Header,
    Query,
}

/// Where OAuth 2.0 puts the token pair (PROMPT.md Appendix E: headers | url).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum OAuth2Location {
    #[default]
    Header,
    Url,
}

/// The auth attached to a collection, folder or request; `Inherit` walks the ancestor chain.
///
/// Dispatched on the `kind` tag of the file's auth object. Unknown kinds land in
/// [`AuthConfig::Unknown`] with everything they carried (the import report lists them), so a
/// file round-trips without inventing a variant. Every typed struct uses
/// `#[serde(default)]`+`flatten(extras)`, so files written by any version of this model stay
/// complete.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub enum AuthConfig {
    #[default]
    Inherit,
    NoAuth,
    ApiKey(Box<ApiKeyAuth>),
    Basic(Box<BasicAuth>),
    Bearer(Box<BearerAuth>),
    Digest(Box<DigestAuth>),
    Hawk(Box<HawkAuth>),
    Jwt(Box<JwtAuth>),
    OAuth1(Box<OAuth1Auth>),
    OAuth2(Box<OAuth2Auth>),
    AwsV4(Box<AwsV4Auth>),
    Ntlm(Box<NtlmAuth>),
    EdgeGrid(Box<EdgeGridAuth>),
    Asap(Box<AsapAuth>),
    /// A kind this model does not know (new protocol, or an import the mapper left alone).
    /// `kind` carries the original tag text, `extras` the rest of the block verbatim.
    Unknown {
        kind: String,
        extras: Extras,
    },
}

impl AuthConfig {
    /// The stable kind id (the serde tag), for UI labels, reports and tests.
    pub fn kind(&self) -> String {
        match self {
            AuthConfig::Inherit => "inherit".into(),
            AuthConfig::NoAuth => "noauth".into(),
            AuthConfig::ApiKey(_) => "apikey".into(),
            AuthConfig::Basic(_) => "basic".into(),
            AuthConfig::Bearer(_) => "bearer".into(),
            AuthConfig::Digest(_) => "digest".into(),
            AuthConfig::Hawk(_) => "hawk".into(),
            AuthConfig::Jwt(_) => "jwt".into(),
            AuthConfig::OAuth1(_) => "oauth1".into(),
            AuthConfig::OAuth2(_) => "oauth2".into(),
            AuthConfig::AwsV4(_) => "awsv4".into(),
            AuthConfig::Ntlm(_) => "ntlm".into(),
            AuthConfig::EdgeGrid(_) => "edgegrid".into(),
            AuthConfig::Asap(_) => "asap".into(),
            AuthConfig::Unknown { kind, .. } => kind.clone(),
        }
    }

    /// True when a send resolves to a concrete credential (not `Inherit`, `NoAuth`, unknown).
    pub fn resolves(&self) -> bool {
        !matches!(self, AuthConfig::Inherit | AuthConfig::NoAuth | AuthConfig::Unknown { .. })
    }

    /// Build from a file's auth object: dispatch on `kind`, preserve the unknown as `Unknown`.
    /// `null` or a missing/blank `kind` means `Inherit`.
    pub fn from_file_value(value: serde_json::Value) -> Result<AuthConfig, String> {
        use serde_json::Value;
        if value.is_null() {
            return Ok(AuthConfig::Inherit);
        }
        let Value::Object(mut map) = value else { return Err("auth must be an object".to_string()) };
        let kind = map.get("kind").and_then(Value::as_str).unwrap_or_default().to_lowercase();
        if kind.is_empty() {
            return Ok(AuthConfig::Inherit);
        }
        let known: Option<AuthConfig> = match kind.as_str() {
            "noauth" => Some(AuthConfig::NoAuth),
            "apikey" => as_typed::<ApiKeyAuth>(&map).map(|t| AuthConfig::ApiKey(Box::new(t))),
            "basic" => as_typed::<BasicAuth>(&map).map(|t| AuthConfig::Basic(Box::new(t))),
            "bearer" => as_typed::<BearerAuth>(&map).map(|t| AuthConfig::Bearer(Box::new(t))),
            "digest" => as_typed::<DigestAuth>(&map).map(|t| AuthConfig::Digest(Box::new(t))),
            "hawk" => as_typed::<HawkAuth>(&map).map(|t| AuthConfig::Hawk(Box::new(t))),
            "jwt" => as_typed::<JwtAuth>(&map).map(|t| AuthConfig::Jwt(Box::new(t))),
            "oauth1" => as_typed::<OAuth1Auth>(&map).map(|t| AuthConfig::OAuth1(Box::new(t))),
            "oauth2" => as_typed::<OAuth2Auth>(&map).map(|t| AuthConfig::OAuth2(Box::new(t))),
            "awsv4" => as_typed::<AwsV4Auth>(&map).map(|t| AuthConfig::AwsV4(Box::new(t))),
            "ntlm" => as_typed::<NtlmAuth>(&map).map(|t| AuthConfig::Ntlm(Box::new(t))),
            "edgegrid" => as_typed::<EdgeGridAuth>(&map).map(|t| AuthConfig::EdgeGrid(Box::new(t))),
            "asap" => as_typed::<AsapAuth>(&map).map(|t| AuthConfig::Asap(Box::new(t))),
            _ => None,
        };
        if let Some(known) = known {
            Ok(known)
        } else {
            map.shift_remove("kind");
            let extras = Extras(map);
            Ok(AuthConfig::Unknown { kind, extras })
        }
    }

    /// The file object for this config, tagged with `kind`; `Unknown` writes its block back
    /// exactly as it arrived.
    pub fn to_file_value(&self) -> serde_json::Value {
        use serde_json::Value;
        let mut map = serde_json::Map::new();
        match self {
            AuthConfig::Inherit => Value::Null,
            AuthConfig::NoAuth => {
                map.insert("kind".into(), Value::String("noauth".into()));
                Value::Object(map)
            }
            AuthConfig::ApiKey(auth) => typed_to_file_value(auth, "apikey"),
            AuthConfig::Basic(auth) => typed_to_file_value(auth, "basic"),
            AuthConfig::Bearer(auth) => typed_to_file_value(auth, "bearer"),
            AuthConfig::Digest(auth) => typed_to_file_value(auth, "digest"),
            AuthConfig::Hawk(auth) => typed_to_file_value(auth, "hawk"),
            AuthConfig::Jwt(auth) => typed_to_file_value(auth, "jwt"),
            AuthConfig::OAuth1(auth) => typed_to_file_value(auth, "oauth1"),
            AuthConfig::OAuth2(auth) => typed_to_file_value(auth, "oauth2"),
            AuthConfig::AwsV4(auth) => typed_to_file_value(auth, "awsv4"),
            AuthConfig::Ntlm(auth) => typed_to_file_value(auth, "ntlm"),
            AuthConfig::EdgeGrid(auth) => typed_to_file_value(auth, "edgegrid"),
            AuthConfig::Asap(auth) => typed_to_file_value(auth, "asap"),
            AuthConfig::Unknown { kind, extras } => {
                let mut map = extras.0.clone();
                map.insert("kind".into(), serde_json::Value::String(kind.clone()));
                serde_json::Value::Object(map)
            }
        }
    }
}

fn typed_to_file_value<T: Serialize>(auth: &T, kind: &str) -> serde_json::Value {
    let mut struct_map = serde_json::to_value(auth)
        .ok()
        .and_then(|v| match v {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();
    struct_map.insert("kind".into(), serde_json::json!(kind));
    serde_json::Value::Object(struct_map)
}

fn as_typed<T: for<'de> Deserialize<'de>>(map: &serde_json::Map<String, serde_json::Value>) -> Option<T> {
    serde_json::from_value(serde_json::Value::Object(map.clone())).ok()
}

impl Serialize for AuthConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_file_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AuthConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        AuthConfig::from_file_value(value).map_err(serde::de::Error::custom)
    }
}

/// API Key (Appendix E): key, value, header or query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyAuth {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub location: AuthLocation,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// Basic (Appendix E): username, password.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BasicAuth {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// Bearer (Appendix E): token, optional prefix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BearerAuth {
    #[serde(default)]
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// Digest (Appendix E): username, password, realm, nonce, nonceCount, algorithm, qop, opaque,
/// clientNonce, disableRetryRequest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DigestAuth {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub realm: String,
    #[serde(default)]
    pub nonce: String,
    #[serde(default)]
    pub nonce_count: String,
    #[serde(default)]
    pub algorithm: DigestAlgorithm,
    #[serde(default)]
    pub qop: String,
    #[serde(default)]
    pub opaque: String,
    #[serde(default)]
    pub client_nonce: String,
    #[serde(default)]
    pub disable_retry_request: bool,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// The hash family the response challenge named (Appendix E list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DigestAlgorithm {
    #[default]
    Md5,
    Md5Sess,
    Sha256,
    Sha256Sess,
    Sha512256,
}

/// Hawk (Appendix C keys): authId, authKey, algorithm, user, nonce, ext, app, dlg, timestamp.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HawkAuth {
    #[serde(default)]
    pub auth_id: String,
    #[serde(default)]
    pub auth_key: String,
    #[serde(default)]
    pub algorithm: HawkAlgorithm,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub nonce: String,
    #[serde(default)]
    pub ext: String,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub dlg: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub include_payload_hash: bool,
    #[serde(default, flatten)]
    pub extras: Extras,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum HawkAlgorithm {
    #[default]
    Sha256,
    Sha1,
}

/// JWT Bearer (Appendix E): header/query, algorithm, secret or private key, payload, prefix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct JwtAuth {
    #[serde(default)]
    pub location: AuthLocation,
    #[serde(default)]
    pub algorithm: JwtAlgorithm,
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub secret_is_base64: bool,
    #[serde(default)]
    pub private_key: String,
    /// The claims JSON the user types; `iat` is added at send unless present (PROMPT.md App E).
    pub payload: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default)]
    pub headers: Vec<JwtHeader>,
    #[serde(default, flatten)]
    pub extras: Extras,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct JwtHeader {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default, flatten)]
    pub extras: Extras,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum JwtAlgorithm {
    #[default]
    Hs256,
    Hs384,
    Hs512,
    Rs256,
    Rs384,
    Rs512,
    Es256,
    Es384,
    Es512,
    Ps256,
    Ps384,
    Ps512,
}

/// OAuth 1.0 (Appendix C keys): consumerKey, consumerSecret, token, tokenSecret,
/// signatureMethod, timestamp, nonce, version, realm, addParamsToHeader, includeBodyHash,
/// addEmptyParamsToSign.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OAuth1Auth {
    /// Appendix E "Add to": request headers or request body/URL.
    #[serde(default)]
    pub add_params_to_header: bool,
    #[serde(default)]
    pub signature_method: OAuth1SignatureMethod,
    #[serde(default)]
    pub consumer_key: String,
    #[serde(default)]
    pub consumer_secret: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub token_secret: String,
    #[serde(default)]
    pub callback_url: String,
    #[serde(default)]
    pub verifier: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub nonce: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub realm: String,
    #[serde(default)]
    pub include_body_hash: bool,
    #[serde(default)]
    pub add_empty_params_to_sign: bool,
    #[serde(default)]
    pub encode_params_in_header: bool,
    #[serde(default, flatten)]
    pub extras: Extras,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum OAuth1SignatureMethod {
    #[default]
    HmacSha1,
    HmacSha256,
    HmacSha512,
    RsaSha1,
    RsaSha256,
    RsaSha512,
    Plaintext,
}

/// Per-parameter delivery choice for OAuth 2.0 extra params (Appendix E).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum OAuth2ParamDelivery {
    #[default]
    Body,
    Url,
    Headers,
}

/// OAuth 2.0 (PROMPT.md 5.3.2 flow + Appendix E fields). Grant-selective fields stay optional;
/// the token flow logic lives in `http` (M5), this is the stored config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2Auth {
    #[serde(default)]
    pub location: OAuth2Location,
    #[serde(default)]
    pub header_prefix: String,
    #[serde(default)]
    pub token_name: String,
    #[serde(default)]
    pub grant_type: OAuth2Grant,
    #[serde(default)]
    pub callback_url: String,
    #[serde(default)]
    pub use_browser: bool,
    #[serde(default)]
    pub auth_url: String,
    #[serde(default)]
    pub token_url: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub client_authentication: OAuth2ClientAuthentication,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pkce: Option<OAuth2Pkce>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub refresh_url: String,
    /// Stored token(s) of the last flow, masked in the UI like secret variables.
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// Unix milliseconds when the access token expires (0 = unknown); set by the token flow.
    #[serde(default)]
    pub access_token_expires_at_ms: i64,
    #[serde(default)]
    pub extra_params: Vec<OAuth2Param>,
    #[serde(default, flatten)]
    pub extras: Extras,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum OAuth2Grant {
    /// Authorization Code with browser + loopback listener (PROMPT.md M5).
    #[default]
    AuthorizationCode,
    AuthorizationCodeWithPkce,
    Implicit,
    PasswordCredentials,
    ClientCredentials,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum OAuth2ClientAuthentication {
    #[default]
    BasicHeader,
    Body,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PkceMethod {
    #[default]
    S256,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2Pkce {
    #[serde(default)]
    pub method: PkceMethod,
    /// The generated verifier, kept between authorize and token requests of one flow.
    #[serde(default)]
    pub verifier: String,
    #[serde(default, flatten)]
    pub extras: Extras,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2Param {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub delivery: OAuth2ParamDelivery,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// AWS Signature v4 (Appendix C/E): accessKey, secretKey, region, service, sessionToken.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AwsV4Auth {
    #[serde(default)]
    pub access_key: String,
    #[serde(default)]
    pub secret_key: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub session_token: String,
    #[serde(default)]
    pub add_auth_data_to_query: bool,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// NTLM (P2; stored now so collections never lose it on round trip).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NtlmAuth {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub workstation: String,
    #[serde(default)]
    pub disable_retry: bool,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// Akamai EdgeGrid (P2; stored now for fidelity).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EdgeGridAuth {
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub client_token: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub nonce: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub base_uri: String,
    #[serde(default)]
    pub headers_to_sign: Vec<String>,
    #[serde(default)]
    pub max_body: String,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// ASAP (Atlassian; P2; stored now for fidelity).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AsapAuth {
    #[serde(default)]
    pub algorithm: JwtAlgorithm,
    #[serde(default)]
    pub issuer: String,
    #[serde(default)]
    pub audience: String,
    #[serde(default)]
    pub key_id: String,
    #[serde(default)]
    pub private_key: String,
    #[serde(default)]
    pub subject: String,
    /// Extra claims as a JSON object the user types; parsed at send.
    #[serde(default)]
    pub additional_claims: String,
    #[serde(default)]
    pub expiry: String,
    #[serde(default, flatten)]
    pub extras: Extras,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tagged_auth_survives_a_round_trip_with_extras() {
        let input = json!({
            "kind": "basic",
            "username": "ada",
            "password": "s3cret",
            "vendorField": {"x": 1}
        });
        let auth: AuthConfig = serde_json::from_value(input).expect("parses");
        match &auth {
            AuthConfig::Basic(basic) => {
                assert_eq!(basic.username, "ada");
                assert_eq!(basic.extras.get("vendorField").map(|v| v.to_string()), Some(r#"{"x":1}"#.to_string()));
            }
            other => panic!("wrong variant: {other:?}"),
        }
        let back = serde_json::to_value(&auth).expect("serializes");
        assert_eq!(back["kind"], json!("basic"));
        assert_eq!(back["username"], json!("ada"));
        assert_eq!(back["vendorField"], json!({"x": 1}));
    }

    #[test]
    fn missing_fields_default_instead_of_failing() {
        let auth: AuthConfig = serde_json::from_value(json!({"kind": "apikey"})).expect("parses");
        match auth {
            AuthConfig::ApiKey(k) => {
                assert_eq!(k.key, "");
                assert_eq!(k.location, AuthLocation::Header);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn unknown_auth_kinds_read_through() {
        let auth: AuthConfig = serde_json::from_value(json!({
            "kind": "acme-hmac-v9",
            "secretConfig": "the-thing"
        }))
        .expect("parses");
        match &auth {
            AuthConfig::Unknown { kind, extras } => {
                assert_eq!(kind, "acme-hmac-v9");
                assert_eq!(extras.iter().count(), 1);
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert!(matches!(auth.kind().as_str(), "acme-hmac-v9"), "{auth:?}");
        assert!(!auth.resolves());
    }

    #[test]
    fn inherit_is_the_default_for_missing_auth() {
        let auth: AuthConfig = serde_json::from_value(json!(null)).expect("parses");
        assert_eq!(auth, AuthConfig::Inherit);
    }
}
