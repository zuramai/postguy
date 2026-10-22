//! Request bodies: none, form-data, urlencoded, raw, binary, GraphQL.
//! Switching the body type in the UI keeps the other modes' data until save; the model only
//! ever persists the active one, so this enum is single-valued by design.

use crate::extras::Extras;
use crate::requests::KeyValue;
use serde::{Deserialize, Serialize};

/// The body of a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "camelCase")]
#[non_exhaustive]
pub enum Body {
    #[default]
    None,
    /// Key-value rows whose values may be text or file references (path, working-dir relative).
    FormData(Vec<KeyValue>),
    /// Percent-encoded at send time.
    UrlEncoded(Vec<KeyValue>),
    Raw {
        #[serde(default)]
        language: RawLanguage,
        #[serde(default)]
        text: String,
        #[serde(default, flatten)]
        extras: Extras,
    },
    /// One file (working-directory-relative path).
    Binary {
        #[serde(default)]
        path: String,
        #[serde(default, flatten)]
        extras: Extras,
    },
    /// Sent as JSON `{"query": ..., "variables": ..., "operationName"?}` with
    /// `Content-Type: application/json`.
    GraphQL {
        #[serde(default)]
        query: String,
        /// Optional name of the operation to run in a multi-operation document.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_name: Option<String>,
        /// The variables JSON the user typed; validity is a hint, never a save blocker.
        #[serde(default)]
        variables: String,
        #[serde(default, flatten)]
        extras: Extras,
    },
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
    fn languages_map_to_content_types_per_the_request_brief() {
        assert_eq!(RawLanguage::Json.content_type(), "application/json");
        assert_eq!(RawLanguage::Text.content_type(), "text/plain");
        assert_eq!(RawLanguage::Html.content_type(), "text/html");
    }
}
