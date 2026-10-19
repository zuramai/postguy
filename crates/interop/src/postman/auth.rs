//! Postman auth blocks ↔ `postguy_model::AuthConfig`.
//!
//! Postman stores an auth as `{type: "<kind>", <kind>: [auth-attribute...]}` where an
//! attribute is `{key, value, type?}`. The model stores a tagged object (e.g.
//! `{kind: "basic", username, password}`), each typed struct's fields in `camelCase`, unknown
//! keys in `extras`. This module does the row→field collapse and the key renames both ways,
//! per Postman Appendix C spellings.

use serde_json::{Map, Value};

use postguy_model::{AuthConfig, Extras};

use super::report::ImportReport;

/// Postman kind names this model has typed structs for. Everything else (`jwt`, `asap`,
/// kinds from newer Postman builds) rides through as [`AuthConfig::Unknown`] with the block
/// verbatim.
const TYPED_KINDS: [&str; 9] = ["apikey", "basic", "bearer", "digest", "hawk", "ntlm", "oauth1", "oauth2", "awsv4"];

/// Collapse Postman auth-attribute rows to a flat `{key: value}` object.
///
/// The LAST non-empty value of a key wins. Non-string attribute values (numbers, booleans,
/// and rare object/array values) keep their raw `Value` — stringifying them would lie and
/// the extras channel carries them back out verbatim.
#[must_use]
pub fn rows_to_map(rows: &Value) -> Map<String, Value> {
    let mut flat = Map::new();
    let record = |flat: &mut Map<String, Value>, key: &str, value: Option<&Value>| {
        let empty_value = value.is_none() || value.is_some_and(|v| v.is_null() || v.as_str().is_some_and(str::is_empty));
        if empty_value {
            // Never erase an earlier non-empty value; the key still ships.
            flat.entry(key.to_string()).or_insert_with(|| Value::String(String::new()));
        } else {
            if let Some(value) = value {
                flat.insert(key.to_string(), value.clone());
            }
        }
    };
    match rows {
        Value::Array(items) => {
            for item in items {
                if let Some(key) = item.get("key").and_then(Value::as_str) {
                    record(&mut flat, key, item.get("value"));
                }
            }
        }
        Value::Object(map) => {
            for (key, value) in map {
                record(&mut flat, key, Some(value));
            }
        }
        _ => {}
    }
    flat
}

/// Reverse of [`rows_to_map`]: the Postman row array (each attribute `{key, value}`).
#[must_use]
pub fn map_to_rows(map: &Map<String, Value>) -> Value {
    Value::Array(
        map.iter()
            .filter(|(_, value)| !(value.as_str().is_some_and(str::is_empty) || matches!(value, Value::Bool(false))))
            .map(|(key, value)| {
                let mut row = Map::new();
                row.insert("key".into(), Value::String(key.clone()));
                if !value.is_null() {
                    row.insert("value".into(), value.clone());
                }
                Value::Object(row)
            })
            .collect(),
    )
}

/// Convert a Postman `type`-tagged auth block into the model auth. Unknown kinds and
/// kinds-with-unmappable-values come back as [`AuthConfig::Unknown`] (verbatim extras,
/// minus `type`) so the block round trips; the report records each such decision.
pub(crate) fn postman_to_auth(auth: &Value, report: &mut ImportReport, path: &str) -> AuthConfig {
    let Some(map) = auth.as_object() else {
        report.normal(path, "auth was not an object; treated as inherited");
        return AuthConfig::Inherit;
    };
    let kind = map.get("type").and_then(Value::as_str).unwrap_or("").to_string();
    if kind == "noauth" {
        return AuthConfig::NoAuth;
    }
    if TYPED_KINDS.contains(&kind.as_str()) {
        let attrs = map.get(&kind).cloned().unwrap_or_else(|| Value::Array(Vec::new()));
        let mut flat = rows_to_map(&attrs);
        match kind.as_str() {
            "apikey" => {
                if let Some(in_value) = flat.shift_remove("in") {
                    match in_value.as_str().map(str::to_ascii_lowercase).as_deref() {
                        Some("header" | "query") => {
                            flat.insert("location".into(), Value::String(knock_location(&in_value)));
                        }
                        _ => {
                            // A weird `in`: keep the original so it round trips verbatim;
                            // the location field stays at its default (header).
                            flat.insert("in".into(), in_value);
                            report.normal(path, "apikey auth has an `in` this model does not know; kept as-is, header assumed");
                        }
                    }
                }
            }
            "digest" => {
                if let Some(spelling) = flat.get("algorithm").and_then(Value::as_str).and_then(digest_algorithm_from_postman) {
                    flat.shift_remove("algorithm");
                    flat.insert("algorithm".into(), Value::String(spelling));
                }
            }
            "hawk" => {
                rename(&mut flat, "extraData", "ext");
                rename(&mut flat, "delegation", "dlg");
                rename(&mut flat, "app", "appId");
            }
            "oauth1" => {
                rename(&mut flat, "accessToken", "token");
                rename(&mut flat, "accessTokenSecret", "tokenSecret");
                if let Some(spelling) = flat.get("signatureMethod").and_then(Value::as_str).and_then(oauth1_signature_from_postman) {
                    flat.shift_remove("signatureMethod");
                    flat.insert("signatureMethod".into(), Value::String(spelling));
                }
            }
            "oauth2" => {
                rename(&mut flat, "accessTokenUrl", "tokenUrl");
                if let Some(add_to) = flat.shift_remove("addTokenTo") {
                    match add_to.as_str().map(str::to_ascii_lowercase).as_deref() {
                        Some("header" | "url") => {
                            flat.insert("location".into(), Value::String(knock_location(&add_to)));
                        }
                        _ => {
                            flat.insert("addTokenTo".into(), add_to);
                            report.normal(path, "oauth2 auth has an `addTokenTo` this model does not know; kept as-is");
                        }
                    }
                }
                if let Some(spelling) = flat.get("grantType").and_then(Value::as_str).and_then(oauth2_grant_from_postman) {
                    flat.shift_remove("grantType");
                    flat.insert("grantType".into(), Value::String(spelling));
                }
            }
            "edgegrid" => {
                rename(&mut flat, "maxBodySize", "maxBody");
            }
            _ => {}
        }
        let mut tagged = Map::new();
        tagged.extend(flat);
        let value = Value::Object(tagged);
        // Typed structs are built directly on the flat attribute map: `kind` is never part
        // of the typed fields, so no helper stripping is needed and empty/absent values
        // take the struct defaults.
        let converted = match kind.as_str() {
            "digest" => serde_json::from_value::<postguy_model::auth::DigestAuth>(value.clone())
                .map(|typed| AuthConfig::Digest(Box::new(typed)))
                .inspect_err(|err| {
                    report.normal(path, err.to_string());
                })
                .ok(),
            "apikey" => {
                serde_json::from_value::<postguy_model::auth::ApiKeyAuth>(value.clone()).map(|typed| AuthConfig::ApiKey(Box::new(typed))).ok()
            }
            "basic" => serde_json::from_value::<postguy_model::auth::BasicAuth>(value.clone()).map(|typed| AuthConfig::Basic(Box::new(typed))).ok(),
            "bearer" => {
                serde_json::from_value::<postguy_model::auth::BearerAuth>(value.clone()).map(|typed| AuthConfig::Bearer(Box::new(typed))).ok()
            }
            "hawk" => serde_json::from_value::<postguy_model::auth::HawkAuth>(value.clone()).map(|typed| AuthConfig::Hawk(Box::new(typed))).ok(),
            "ntlm" => serde_json::from_value::<postguy_model::auth::NtlmAuth>(value.clone()).map(|typed| AuthConfig::Ntlm(Box::new(typed))).ok(),
            "oauth1" => {
                serde_json::from_value::<postguy_model::auth::OAuth1Auth>(value.clone()).map(|typed| AuthConfig::OAuth1(Box::new(typed))).ok()
            }
            "oauth2" => {
                serde_json::from_value::<postguy_model::auth::OAuth2Auth>(value.clone()).map(|typed| AuthConfig::OAuth2(Box::new(typed))).ok()
            }
            "awsv4" => serde_json::from_value::<postguy_model::auth::AwsV4Auth>(value.clone()).map(|typed| AuthConfig::AwsV4(Box::new(typed))).ok(),
            _ => None,
        };
        if let Some(auth) = converted {
            if matches!(&auth, AuthConfig::Unknown { .. }) {
                // A known kind whose values did not fit the typed fields (e.g. a digest
                // algorithm spelling we do not know): keep it verbatim instead.
                report.normal(path, "auth block kept verbatim: a value did not fit the typed fields");
            }
            return auth;
        }
    }
    // jwt / asap / unknown kinds: verbatim under extras (minus `type`).
    let mut extras = map.clone();
    extras.shift_remove("type");
    report.normal(path, "auth kind not in the typed set; kept verbatim");
    AuthConfig::Unknown { kind, extras: Extras(extras) }
}

/// The Postman auth block for a model auth (row form); `noauth` returns
/// `{"type": "noauth"}` and `Inherit` returns nothing (the caller omits the key).
#[must_use]
pub fn auth_to_postman(auth: &AuthConfig) -> Option<Value> {
    if let AuthConfig::Unknown { kind, extras } = auth {
        // Verbatim block: `type` back on, everything else exactly as it arrived.
        let mut out = extras.0.clone();
        out.shift_remove("type");
        let mut with_type = Map::new();
        with_type.insert("type".into(), Value::String(kind.clone()));
        for (key, value) in out {
            with_type.insert(key, value);
        }
        return Some(Value::Object(with_type));
    }
    let file_value = auth.to_file_value();
    let map = file_value.as_object()?;
    let kind = map.get("kind")?.as_str()?.to_string();
    if kind == "noauth" {
        let mut out = Map::new();
        out.insert("type".into(), Value::String("noauth".into()));
        return Some(Value::Object(out));
    }
    let mut attrs = map.clone();
    attrs.shift_remove("kind");
    match kind.as_str() {
        "apikey" => {
            // A literal weird `in` the extras kept wins; otherwise the location spells `in`.
            let location = attrs.shift_remove("location");
            if !attrs.contains_key("in") && location.is_some() {
                let word = match location.as_ref().and_then(Value::as_str) {
                    Some("query") => "query",
                    _ => "header",
                };
                attrs.insert("in".into(), Value::String(word.to_string()));
            }
        }
        "digest" => {
            if let Some(Value::String(algorithm)) = attrs.get("algorithm").cloned() {
                attrs.shift_remove("algorithm");
                attrs.insert("algorithm".into(), Value::String(digest_algorithm_to_postman(&algorithm)));
            }
        }
        "hawk" => {
            rename_back(&mut attrs, "ext", "extraData");
            rename_back(&mut attrs, "dlg", "delegation");
            rename_back(&mut attrs, "appId", "appId");
        }
        "oauth1" => {
            rename_back(&mut attrs, "token", "accessToken");
            rename_back(&mut attrs, "tokenSecret", "accessTokenSecret");
            if let Some(Value::String(method)) = attrs.get("signatureMethod").cloned() {
                attrs.shift_remove("signatureMethod");
                attrs.insert("signatureMethod".into(), Value::String(oauth1_signature_to_postman(&method)));
            }
        }
        "oauth2" => {
            if let Some(Value::String(grant)) = attrs.get("grantType").cloned() {
                attrs.shift_remove("grantType");
                attrs.insert("grantType".into(), Value::String(oauth2_grant_to_postman(&grant)));
            }
            rename_back(&mut attrs, "tokenUrl", "accessTokenUrl");
            let location = attrs.shift_remove("location").map(|loc| match loc.as_str() {
                Some("url") => "url",
                _ => "header",
            });
            if let Some(word) = location {
                attrs.insert("addTokenTo".into(), Value::String(word.to_string()));
            }
            // Fields without a Postman-side meaning drop out.
            attrs.shift_remove("pkce");
            attrs.shift_remove("refreshUrl");
            attrs.shift_remove("accessToken");
            attrs.shift_remove("refreshToken");
            attrs.shift_remove("extraParams");
        }
        _ => {}
    }
    let mut out = Map::new();
    out.insert("type".into(), Value::String(kind.clone()));
    out.insert(kind, map_to_rows(&attrs));
    Some(Value::Object(out))
}

/// Rename a key in the flat attribute map (Postman spelling → model spelling), keeping the
/// first value only.
fn rename(map: &mut Map<String, Value>, from: &str, to: &str) {
    let value = map.shift_remove(from);
    if let Some(value) = value {
        map.entry(to.to_string()).or_insert(value);
    }
}

/// Same on the way out (model spelling → Postman spelling), always replacing.
fn rename_back(map: &mut Map<String, Value>, from: &str, to: &str) {
    let value = map.shift_remove(from);
    if let Some(value) = value {
        map.insert(to.to_string(), value);
    }
}

/// The Postman word for a location value, lowercase-normalized then spelled as the model
/// expects (`header` | `query` | `url`).
fn knock_location(word: &Value) -> String {
    let word = word.as_str().unwrap_or("header").to_ascii_lowercase();
    // Only header/query/url are valid; anything else was screened before this call.
    let word = word.as_str();
    word.to_string()
}

/// Digest algorithm spellings Postman writes (canonical form), dash/case tolerant on the
/// way in. `None` keeps the original text (the block then stays Unknown or carries the raw
/// string in its extras).
fn digest_algorithm_from_postman(spelling: &str) -> Option<String> {
    let normalized = spelling.to_ascii_lowercase().replace(['-', ' ', '_'], "");
    let out = match normalized.as_str() {
        "md5" => "md5".to_string(),
        "md5sess" => "md5Sess".to_string(),
        "sha256" => "sha256".to_string(),
        "sha256sess" => "sha256Sess".to_string(),
        "sha512256" => "sha512256".to_string(),
        _ => return None,
    };
    Some(out.to_string())
}

fn digest_algorithm_to_postman(model: &str) -> String {
    match model.to_ascii_lowercase().as_str() {
        "md5" => "MD5".to_string(),
        "md5sess" => "MD5-sess".to_string(),
        "sha256" => "SHA-256".to_string(),
        "sha256sess" => "SHA-256-sess".to_string(),
        "sha512256" => "SHA-512-256".to_string(),
        other => other.to_string(),
    }
}

fn oauth1_signature_from_postman(spelling: &str) -> Option<String> {
    let normalized = spelling.to_ascii_lowercase().replace(['-', ' ', '_'], "");
    let out = match normalized.as_str() {
        "hmacsha1" => "hmacSha1",
        "hmacsha256" => "hmacSha256",
        "hmacsha512" => "hmacSha512",
        "rsasha1" => "rsaSha1",
        "rsasha256" => "rsaSha256",
        "rsasha512" => "rsaSha512",
        "plaintext" => "plaintext",
        _ => return None,
    };
    Some(out.to_string())
}

fn oauth1_signature_to_postman(model: &str) -> String {
    match model.to_ascii_lowercase().replace(['-', ' ', '_'], "").as_str() {
        "hmacsha1" => "HMAC-SHA1".to_string(),
        "hmacsha256" => "HMAC-SHA256".to_string(),
        "hmacsha512" => "HMAC-SHA512".to_string(),
        "rsasha1" => "RSA-SHA1".to_string(),
        "rsasha256" => "RSA-SHA256".to_string(),
        "rsasha512" => "RSA-SHA512".to_string(),
        "plaintext" => "PLAINTEXT".to_string(),
        other => other.to_string(),
    }
}

fn oauth2_grant_from_postman(spelling: &str) -> Option<String> {
    let normalized = spelling.to_ascii_lowercase().replace(['-', ' ', '_'], "");
    let out = match normalized.as_str() {
        "authorizationcode" => "authorizationCode",
        "authorizationcodewithpkce" => "authorizationCodeWithPkce",
        "implicit" => "implicit",
        "password" | "passwordcredentials" => "passwordCredentials",
        "clientcredentials" => "clientCredentials",
        _ => return None,
    };
    Some(out.to_string())
}

fn oauth2_grant_to_postman(model: &str) -> String {
    match model.to_ascii_lowercase().replace(['-', ' ', '_'], "").as_str() {
        "authorizationcode" => "authorization_code".to_string(),
        "authorizationcodewithpkce" => "authorization_code_with_pkce".to_string(),
        "implicit" => "implicit".to_string(),
        "passwordcredentials" => "password_credentials".to_string(),
        "clientcredentials" => "client_credentials".to_string(),
        other => other.to_string(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::postman::report::ImportReport;

    /// `Some(auth)` for the ApiKey variant; tests use `ok_or(Err)` to report mismatched
    /// kinds without `panic!` (the crate denies panic paths even in tests).
    fn key_of(auth: &AuthConfig) -> Option<&postguy_model::auth::ApiKeyAuth> {
        match auth {
            AuthConfig::ApiKey(key) => Some(key),
            _ => None,
        }
    }

    fn basic_of(auth: &AuthConfig) -> Option<&postguy_model::auth::BasicAuth> {
        match auth {
            AuthConfig::Basic(basic) => Some(basic),
            _ => None,
        }
    }

    fn digest_of(auth: &AuthConfig) -> Option<&postguy_model::auth::DigestAuth> {
        match auth {
            AuthConfig::Digest(digest) => Some(digest),
            _ => None,
        }
    }

    fn unknown_of(auth: &AuthConfig) -> Option<(&String, &Extras)> {
        match auth {
            AuthConfig::Unknown { kind, extras } => Some((kind, extras)),
            _ => None,
        }
    }

    #[test]
    fn rows_collapse_to_the_last_non_empty_value() {
        let rows = serde_json::json!([
            {"key": "username", "value": "first"},
            {"key": "username", "value": ""},
            {"key": "username", "value": "ada"},
            {"key": "password", "value": "p1"},
            {"key": "password"}
        ]);
        let flat = rows_to_map(&rows);
        assert_eq!(flat.get("username").and_then(Value::as_str), Some("ada"));
        assert_eq!(flat.get("password").and_then(Value::as_str), Some("p1"));
        let back = map_to_rows(&flat);
        assert_eq!(back.to_string(), r#"[{"key":"username","value":"ada"},{"key":"password","value":"p1"}]"#);
    }

    #[test]
    fn basic_auth_maps_both_ways() -> Result<(), String> {
        let mut report = ImportReport::default();
        let postman = serde_json::json!({
            "type": "basic",
            "basic": [
                {"key": "username", "value": "ada"},
                {"key": "password", "value": "s3cret"}
            ]
        });
        let auth = postman_to_auth(&postman, &mut report, "x");
        let basic = basic_of(&auth).ok_or("must map as basic")?;
        assert_eq!(basic.username, "ada");
        assert!(basic.extras.is_empty());
        assert_eq!(auth_to_postman(&auth), Some(postman));
        Ok(())
    }

    /// The apikey `in` filter: known spellings become the location; weird spellings ride
    /// the extras channel (the location stays at its header default).
    #[test]
    fn apikey_location_and_weird_in_round_trip() -> Result<(), String> {
        let mut report = ImportReport::default();
        let clean = serde_json::json!({"type": "apikey", "apikey": [
            {"key": "key", "value": "X-Api-Key"}, {"key": "value", "value": "abc"}, {"key": "in", "value": "query"}]});
        let auth = postman_to_auth(&clean, &mut report, "x");
        let key = key_of(&auth).ok_or("must map as apikey")?;
        assert!(matches!(key.location, postguy_model::auth::AuthLocation::Query), "in: query → location query");
        assert_eq!(auth_to_postman(&auth), Some(clean));

        let weird = serde_json::json!({"type": "apikey", "apikey": [
            {"key": "key", "value": "X-Api-Key"}, {"key": "in", "value": "in-query-weird"}]});
        let auth = postman_to_auth(&weird, &mut report, "x");
        let key = key_of(&auth).ok_or("still an apikey")?;
        assert!(matches!(key.location, postguy_model::auth::AuthLocation::Header), "weird in keeps the default");
        assert_eq!(key.extras.get("in").and_then(Value::as_str), Some("in-query-weird"));
        assert_eq!(auth_to_postman(&auth), Some(weird), "weird `in` round trips");
        Ok(())
    }

    #[test]
    fn jwt_stays_unknown_and_round_trips_verbatim() -> Result<(), String> {
        let mut report = ImportReport::default();
        let jwt = serde_json::json!({"type": "jwt", "jwt": [
            {"key": "algorithm", "value": "HS256"}, {"key": "payload", "value": "{}"}]});
        let auth = postman_to_auth(&jwt, &mut report, "x");
        let (kind, extras) = unknown_of(&auth).ok_or("jwt must stay Unknown")?;
        assert_eq!(kind, "jwt");
        assert_eq!(extras.0.get("jwt"), Some(&jwt["jwt"]));
        assert!(report.warnings.iter().any(|w| w.contains("kept verbatim")));
        assert_eq!(auth_to_postman(&auth), Some(jwt));
        Ok(())
    }

    #[test]
    fn digest_algorithm_spellings_map_both_ways() -> Result<(), String> {
        let mut report = ImportReport::default();
        let postman = serde_json::json!({"type": "digest", "digest": [
            {"key": "digestAlgorithm", "value": "IGNORED"},
            {"key": "algorithm", "value": "MD5-sess"},
            {"key": "disableRetryRequest", "value": true}]});
        let auth = postman_to_auth(&postman, &mut report, "x");
        let digest = digest_of(&auth)
            .ok_or_else(|| format!("digest must map when the algorithm is known: warnings={:?} kind={}", report.warnings, auth.kind()))?;
        assert!(matches!(digest.algorithm, postguy_model::auth::DigestAlgorithm::Md5Sess));
        assert!(digest.disable_retry_request);
        let back = auth_to_postman(&auth).ok_or("digest must export")?;
        let rows: Result<Value, String> = match back.get("digest") {
            Some(Value::Array(items)) => items
                .iter()
                .find(|row| row.get("key").and_then(Value::as_str) == Some("algorithm"))
                .cloned()
                .ok_or_else(|| "algorithm row missing".to_string()),
            other => Err(format!("digest is not rows: {other:?}")),
        };
        let row = rows?;
        assert_eq!(row.get("value").and_then(Value::as_str), Some("MD5-sess"), "canonical digest spelling: {back}");
        Ok(())
    }
}
