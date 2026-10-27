//! Plain-data description of a loaded gRPC service definition: services, methods and the
//! fields of every reachable message. Built by `postguy-grpc` from a descriptor pool and read by
//! the UI and automation, which never touch descriptors themselves.

use std::collections::BTreeMap;

use postguy_util::json::{JsonPathSegment, object_keys};
use serde::{Deserialize, Serialize};

/// Services, methods and messages of one service definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrpcSchema {
    /// The `.proto` file names the definition came from (well-known types left out).
    pub files: Vec<String>,
    pub services: Vec<GrpcServiceInfo>,
    /// Every message reachable from a method input or output, by full name.
    pub messages: BTreeMap<String, GrpcMessageInfo>,
}

/// One service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrpcServiceInfo {
    /// Full name, for example `echo.Echo`.
    pub name: String,
    pub methods: Vec<GrpcMethodInfo>,
}

/// One method of a service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrpcMethodInfo {
    pub name: String,
    /// `package.Service/Method`.
    pub path: String,
    /// Full name of the input message.
    pub input: String,
    /// Full name of the output message.
    pub output: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
}

impl GrpcMethodInfo {
    /// Short label of the call shape: `unary`, `server stream`, `client stream` or `bidi stream`.
    pub fn kind_label(&self) -> &'static str {
        match (self.client_streaming, self.server_streaming) {
            (false, false) => "unary",
            (false, true) => "server stream",
            (true, false) => "client stream",
            (true, true) => "bidi stream",
        }
    }
}

/// One message type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrpcMessageInfo {
    /// Full name, for example `echo.EchoMessage`.
    pub name: String,
    pub fields: Vec<GrpcFieldInfo>,
}

/// One field of a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrpcFieldInfo {
    pub name: String,
    pub json_name: String,
    pub number: u32,
    /// Display text, for example `string`, `repeated int32`, `map<string, Label>` or `Result`.
    pub type_label: String,
    /// Full name of the message or enum type, or the scalar name.
    pub type_full: String,
    /// Full name of the message whose keys appear under this field (the field's message type,
    /// or a map's value message). `None` for scalars, enums and `google.protobuf.*` types.
    pub message: Option<String>,
    pub repeated: bool,
    pub map: bool,
    /// The (non-synthetic) oneof this field belongs to.
    pub oneof: Option<String>,
}

impl GrpcFieldInfo {
    fn matches(&self, key: &str) -> bool {
        self.name == key || self.json_name == key
    }
}

/// A key in a message's JSON that the message does not define.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UnknownField {
    /// Byte offset of the key token, opening quote included.
    pub start: usize,
    /// Byte offset just past the closing quote.
    pub end: usize,
    pub key: String,
    /// Full name of the message the key was checked against.
    pub message: String,
}

/// Where a scanned key sits relative to the schema.
#[derive(Clone, Copy)]
enum Scope<'a> {
    /// The document root: an object of `message`, or an array of them (client streaming).
    Root(&'a str),
    Message(&'a str),
    /// A map field's object; its keys are map keys, its values the map's value message.
    MapObject(Option<&'a str>),
    /// A repeated field's array of the element message.
    List(Option<&'a str>),
    /// Nothing below is checked (scalar, enum, well-known type, unknown parent).
    Skip,
}

impl GrpcSchema {
    /// The service and method for `package.Service/Method` (or `package.Service.Method`).
    pub fn method(&self, path: &str) -> Option<(&GrpcServiceInfo, &GrpcMethodInfo)> {
        let path = path.trim().trim_start_matches('/');
        if path.is_empty() {
            return None;
        }
        self.services.iter().find_map(|service| {
            service
                .methods
                .iter()
                .find(|method| {
                    method.path == path
                        || path.strip_prefix(service.name.as_str()).and_then(|rest| rest.strip_prefix('.')) == Some(method.name.as_str())
                })
                .map(|method| (service, method))
        })
    }

    /// Keys in `text` (JSON for `message`, possibly half-typed) that the schema does not define.
    /// Map keys, well-known-type contents and keys under an unknown parent are never reported.
    pub fn unknown_fields(&self, message: &str, text: &str) -> Vec<UnknownField> {
        if !self.messages.contains_key(message) {
            return Vec::new();
        }
        let mut unknown = Vec::new();
        for key in object_keys(text) {
            let mut scope = Scope::Root(message);
            for segment in &key.path {
                scope = self.step(scope, segment);
            }
            let (Scope::Root(owner) | Scope::Message(owner)) = scope else { continue };
            let Some(info) = self.messages.get(owner) else { continue };
            if !info.fields.iter().any(|field| field.matches(&key.name)) {
                unknown.push(UnknownField { start: key.range.start, end: key.range.end, key: key.name, message: owner.to_owned() });
            }
        }
        unknown
    }

    fn step<'a>(&'a self, scope: Scope<'a>, segment: &JsonPathSegment) -> Scope<'a> {
        let message_scope = |name: Option<&'a str>| name.map_or(Scope::Skip, Scope::Message);
        match (scope, segment) {
            (Scope::Root(owner), JsonPathSegment::Index) => Scope::Message(owner),
            (Scope::Root(owner) | Scope::Message(owner), JsonPathSegment::Key(key)) => {
                let Some(field) = self.messages.get(owner).and_then(|info| info.fields.iter().find(|field| field.matches(key))) else {
                    return Scope::Skip;
                };
                let inner = field.message.as_deref();
                if field.map {
                    Scope::MapObject(inner)
                } else if field.repeated {
                    Scope::List(inner)
                } else {
                    message_scope(inner)
                }
            }
            (Scope::MapObject(value), JsonPathSegment::Key(_)) => message_scope(value),
            (Scope::List(element), JsonPathSegment::Index) => message_scope(element),
            _ => Scope::Skip,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, json_name: &str, message: Option<&str>, repeated: bool, map: bool) -> GrpcFieldInfo {
        GrpcFieldInfo { name: name.into(), json_name: json_name.into(), message: message.map(Into::into), repeated, map, ..Default::default() }
    }

    fn schema() -> GrpcSchema {
        let mut messages = BTreeMap::new();
        messages.insert(
            "t.Req".to_owned(),
            GrpcMessageInfo {
                name: "t.Req".into(),
                fields: vec![
                    field("user_name", "userName", None, false, false),
                    field("inner", "inner", Some("t.Inner"), false, false),
                    field("items", "items", Some("t.Inner"), true, false),
                    field("labels", "labels", Some("t.Inner"), false, true),
                    field("tags", "tags", None, false, true),
                    field("when", "when", None, false, false),
                ],
            },
        );
        messages.insert("t.Inner".to_owned(), GrpcMessageInfo { name: "t.Inner".into(), fields: vec![field("x", "x", None, false, false)] });
        GrpcSchema {
            files: vec!["t.proto".into()],
            services: vec![GrpcServiceInfo {
                name: "t.Svc".into(),
                methods: vec![GrpcMethodInfo {
                    name: "Do".into(),
                    path: "t.Svc/Do".into(),
                    input: "t.Req".into(),
                    output: "t.Inner".into(),
                    ..Default::default()
                }],
            }],
            messages,
        }
    }

    fn unknown(text: &str) -> Vec<(String, String)> {
        schema().unknown_fields("t.Req", text).into_iter().map(|u| (u.key, u.message)).collect()
    }

    #[test]
    fn method_accepts_slash_and_dot_forms() {
        let schema = schema();
        assert_eq!(schema.method("t.Svc/Do").map(|(_, m)| m.name.as_str()), Some("Do"));
        assert_eq!(schema.method("t.Svc.Do").map(|(_, m)| m.name.as_str()), Some("Do"));
        assert!(schema.method("t.Svc/Nope").is_none());
        assert!(schema.method("").is_none());
    }

    #[test]
    fn flags_unknown_top_level_and_nested_keys_with_ranges() {
        let text = r#"{"user_name": "a", "bogus": 1, "inner": {"x": 1, "y": 2}}"#;
        let found = schema().unknown_fields("t.Req", text);
        assert_eq!(found.len(), 2);
        assert_eq!(&text[found[0].start..found[0].end], "\"bogus\"");
        assert_eq!((found[0].message.as_str(), found[1].key.as_str(), found[1].message.as_str()), ("t.Req", "y", "t.Inner"));
    }

    #[test]
    fn proto_and_json_names_are_both_accepted() {
        assert!(unknown(r#"{"user_name": "a", "userName": "b"}"#).is_empty());
    }

    #[test]
    fn map_keys_are_not_flagged_but_map_values_are_checked() {
        assert!(unknown(r#"{"tags": {"anything": "v"}}"#).is_empty());
        assert_eq!(unknown(r#"{"labels": {"free": {"x": 1, "z": 2}}}"#), vec![("z".into(), "t.Inner".into())]);
    }

    #[test]
    fn repeated_elements_and_top_level_arrays_are_checked() {
        assert_eq!(unknown(r#"{"items": [{"x": 1}, {"q": 1}]}"#), vec![("q".into(), "t.Inner".into())]);
        assert_eq!(unknown(r#"[{"user_name": "a"}, {"nope": 1}]"#), vec![("nope".into(), "t.Req".into())]);
    }

    #[test]
    fn well_known_and_unknown_parents_are_not_descended() {
        assert!(unknown(r#"{"when": {"seconds": 1, "whatever": 2}}"#).is_empty());
        assert_eq!(unknown(r#"{"bogus": {"deep": 1}}"#), vec![("bogus".into(), "t.Req".into())]);
    }

    #[test]
    fn half_typed_text_and_variables_do_not_mis_flag() {
        assert!(unknown(r#"{"te"#).is_empty());
        assert!(unknown(r#"{"user_name": {{name}}, "inner": {"x": "{{v}}""#).is_empty());
        assert!(schema().unknown_fields("t.Missing", r#"{"a": 1}"#).is_empty());
    }
}
