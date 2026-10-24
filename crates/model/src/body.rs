//! Request bodies: none, form-data, urlencoded, raw, binary, GraphQL.
//! Switching the body type in the UI keeps the other modes' data until save; the model only
//! ever persists the active one, so this enum is single-valued by design.

use crate::extras::Extras;
use crate::requests::KeyValue;
use serde::{Deserialize, Serialize};

/// The body of a request.
///
/// On disk it is internally tagged (`{"type": "formData", "rows": [...]}`). serde cannot
/// serialize an internally tagged *newtype* variant that holds a sequence, so the file shape is
/// spelled out by [`BodyRepr`] and the public enum keeps its ergonomic tuple variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(from = "BodyRepr", into = "BodyRepr")]
#[non_exhaustive]
pub enum Body {
    #[default]
    None,
    /// Key-value rows whose values may be text or file references (path, working-dir relative).
    FormData(Vec<KeyValue>),
    /// Percent-encoded at send time.
    UrlEncoded(Vec<KeyValue>),
    Raw {
        language: RawLanguage,
        text: String,
        extras: Extras,
    },
    /// One file (working-directory-relative path).
    Binary {
        path: String,
        extras: Extras,
    },
    /// Sent as JSON `{"query": ..., "variables": ..., "operationName"?}` with
    /// `Content-Type: application/json`.
    GraphQL {
        query: String,
        /// Optional name of the operation to run in a multi-operation document.
        operation_name: Option<String>,
        /// The variables JSON the user typed; validity is a hint, never a save blocker.
        variables: String,
        extras: Extras,
    },
}

/// The serialized form of [`Body`].
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum BodyRepr {
    None,
    FormData {
        #[serde(default)]
        rows: Vec<KeyValue>,
    },
    UrlEncoded {
        #[serde(default)]
        rows: Vec<KeyValue>,
    },
    Raw {
        #[serde(default)]
        language: RawLanguage,
        #[serde(default)]
        text: String,
        #[serde(default, flatten)]
        extras: Extras,
    },
    Binary {
        #[serde(default)]
        path: String,
        #[serde(default, flatten)]
        extras: Extras,
    },
    GraphQL {
        #[serde(default)]
        query: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_name: Option<String>,
        #[serde(default)]
        variables: String,
        #[serde(default, flatten)]
        extras: Extras,
    },
}

impl From<BodyRepr> for Body {
    fn from(repr: BodyRepr) -> Body {
        match repr {
            BodyRepr::None => Body::None,
            BodyRepr::FormData { rows } => Body::FormData(rows),
            BodyRepr::UrlEncoded { rows } => Body::UrlEncoded(rows),
            BodyRepr::Raw { language, text, extras } => Body::Raw { language, text, extras },
            BodyRepr::Binary { path, extras } => Body::Binary { path, extras },
            BodyRepr::GraphQL { query, operation_name, variables, extras } => Body::GraphQL { query, operation_name, variables, extras },
        }
    }
}

impl From<Body> for BodyRepr {
    fn from(body: Body) -> BodyRepr {
        match body {
            Body::None => BodyRepr::None,
            Body::FormData(rows) => BodyRepr::FormData { rows },
            Body::UrlEncoded(rows) => BodyRepr::UrlEncoded { rows },
            Body::Raw { language, text, extras } => BodyRepr::Raw { language, text, extras },
            Body::Binary { path, extras } => BodyRepr::Binary { path, extras },
            Body::GraphQL { query, operation_name, variables, extras } => BodyRepr::GraphQL { query, operation_name, variables, extras },
        }
    }
}

impl Body {
    /// True when the user has to type data for this body to mean anything.
    pub fn needs_data(&self) -> bool {
        !matches!(self, Body::None)
    }

    /// A dot indicator for the Body tab: non-none body shows a dot.
    pub fn is_non_none(&self) -> bool {
        self.needs_data()
    }
}

/// The `raw` body language dropdown. Controls the default Content-Type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum RawLanguage {
    #[default]
    Text,
    Json,
    Xml,
    Html,
    JavaScript,
}

impl RawLanguage {
    /// The Content-Type this language sets when the user set none.
    pub fn content_type(&self) -> &'static str {
        match self {
            RawLanguage::Text => "text/plain",
            RawLanguage::Json => "application/json",
            RawLanguage::Xml => "application/xml",
            RawLanguage::Html => "text/html",
            RawLanguage::JavaScript => "application/javascript",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tagged_body_round_trips_and_keeps_extras() {
        let input = json!({"type": "raw", "language": "json", "text": "{\"a\": 1}", "editor": "folded-line-3"});
        let body: Body = serde_json::from_value(input).expect("parses");
        match &body {
            Body::Raw { language, text, extras } => {
                assert_eq!(*language, RawLanguage::Json);
                assert_eq!(text, "{\"a\": 1}");
                assert_eq!(extras.iter().count(), 1);
            }
            other => panic!("wrong variant: {other:?}"),
        }
        let back = serde_json::to_value(&body).expect("serializes");
        assert_eq!(back["type"], json!("raw"));
        assert_eq!(back["editor"], json!("folded-line-3"));
    }

    #[test]
    fn graphql_body_stores_everything_the_editor_needs() {
        let input = json!({"type": "graphQL", "query": "{ me }", "variables": "{}"});
        let body: Body = serde_json::from_value(input).expect("parses");
        match &body {
            Body::GraphQL { query, .. } => assert_eq!(query, "{ me }"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn every_body_variant_serializes_and_round_trips() {
        // serde cannot serialize an internally tagged newtype variant holding a sequence, which
        // made every request with a form-data or url-encoded body fail to save.
        let rows = vec![KeyValue { key: "a".into(), value: "1".into(), ..KeyValue::default() }, KeyValue { key: "b".into(), ..KeyValue::default() }];
        let bodies = [
            Body::None,
            Body::FormData(rows.clone()),
            Body::UrlEncoded(rows),
            Body::FormData(Vec::new()),
            Body::Raw { language: RawLanguage::Json, text: "{}".into(), extras: Extras::default() },
            Body::Binary { path: "f.bin".into(), extras: Extras::default() },
            Body::GraphQL { query: "{ me }".into(), operation_name: Some("Me".into()), variables: "{}".into(), extras: Extras::default() },
        ];
        for body in bodies {
            let value = serde_json::to_value(&body).expect("serializes");
            let back: Body = serde_json::from_value(value.clone()).expect("parses");
            assert_eq!(back, body, "{value}");
        }
        let value = serde_json::to_value(Body::UrlEncoded(vec![KeyValue { key: "k".into(), ..KeyValue::default() }])).expect("serializes");
        assert_eq!(value["type"], json!("urlEncoded"));
        assert_eq!(value["rows"][0]["key"], json!("k"));
    }

    #[test]
    fn languages_map_to_content_types_per_the_request_brief() {
        assert_eq!(RawLanguage::Json.content_type(), "application/json");
        assert_eq!(RawLanguage::Text.content_type(), "text/plain");
        assert_eq!(RawLanguage::Html.content_type(), "text/html");
    }
}
