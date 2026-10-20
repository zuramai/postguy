//! Request models (PROMPT.md 4.5, 5.3) and their shared row/script types.

use crate::auth::AuthConfig;
use crate::body::Body;
use crate::extras::Extras;
use crate::ids::Id;
use serde::{Deserialize, Serialize};

/// One key-value row (PROMPT.md 4.5): the table type used by params, headers, form-data fields
/// and gRPC metadata. `kind` matters for form-data rows (text vs file); file paths are
/// working-directory-relative (PROMPT.md 5.3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyValue {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "true_value")]
    pub enabled: bool,
    #[serde(default)]
    pub kind: KeyValueKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default)]
    pub secret: bool,
    #[serde(default, flatten)]
    pub extras: Extras,
}

impl Default for KeyValue {
    /// A new row is enabled, matching the file default (`enabled` missing means true).
    fn default() -> Self {
        KeyValue {
            key: String::new(),
            value: String::new(),
            description: String::new(),
            enabled: true,
            kind: KeyValueKind::Text,
            content_type: None,
            secret: false,
            extras: Extras::default(),
        }
    }
}

impl KeyValue {
    /// A single empty row as a table's trailing entry (`Add variable` placeholder semantics).
    pub fn empty_enabled() -> Self {
        Self::default()
    }

    /// Whether the row participates when built (an empty-key row is kept but ignored —
    /// PROMPT.md 5.3.2).
    pub fn contributes(&self) -> bool {
        self.enabled && !self.key.is_empty()
    }
}

fn true_value() -> bool {
    true
}

/// Text row or file row (form-data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum KeyValueKind {
    #[default]
    Text,
    File,
}

/// The HTTP methods v1 speaks (PROMPT.md 5.1 sidebar labels; custom methods are P2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl HttpMethod {
    /// The sidebar short label (PROMPT.md 5.1): GET, POST, ..., DEL, HEAD, OPT.
    pub fn short_label(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DEL",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPT",
        }
    }
}

/// A script as the user typed it; `which` phase it runs in is the containing struct's job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Script {
    #[serde(default)]
    pub text: String,
    #[serde(default, flatten)]
    pub extras: Extras,
}

impl Script {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// Pre-request / test scripts, plus the gRPC phases (PROMPT.md 5.6.1: gRPC adds before
/// invoke, on message, after response). `test` is the on-disk event name used by Postman
/// (PROMPT.md 5.6.1); ui and engine pick the phases that exist for the kind at hand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Scripts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerequest: Option<Script>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<Script>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_invoke: Option<Script>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_message: Option<Script>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_response: Option<Script>,
    #[serde(default, flatten)]
    pub extras: Extras,
}

impl Scripts {
    pub fn is_empty(&self) -> bool {
        self.iter().all(|s| s.is_empty())
    }

    /// All present scripts, in no guaranteed order, for dot indicators and reports.
    pub fn iter(&self) -> impl Iterator<Item = &Script> {
        [self.prerequest.as_ref(), self.test.as_ref(), self.before_invoke.as_ref(), self.on_message.as_ref(), self.after_response.as_ref()]
            .into_iter()
            .flatten()
    }
}

/// The per-protocol request payload of a saved request; tagged for file readability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[non_exhaustive]
pub enum RequestKind {
    Http(HttpRequest),
    Grpc(GrpcRequest),
    WebSocket(WsRequest),
}

/// Manual Default for the same reason as `#[default]`: never derivable for a data variant.
impl Default for RequestKind {
    /// A plain default HTTP request (new request flow: HTTP first, PROMPT.md 6.2).
    fn default() -> Self {
        RequestKind::Http(HttpRequest::default())
    }
}

impl RequestKind {
    /// A stable short protocol id for UI and reports ("http" | "grpc" | "websocket").
    pub fn protocol(&self) -> &'static str {
        match self {
            RequestKind::Http(_) => "http",
            RequestKind::Grpc(_) => "grpc",
            RequestKind::WebSocket(_) => "websocket",
        }
    }
}

/// A saved response (example) by itself: PROMPT.md 4.5. `original_request` is a snapshot so
/// the example can be opened as a request without touching the live one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Example {
    #[serde(default)]
    pub id: Id,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub original_request: Option<RequestKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub cookies: Vec<KeyValue>,
    /// Body bytes and their declared encoding (compression), as `store` persisted them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ExampleBody>,
    /// The language the Pretty view uses (PROMPT.md 5.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<crate::body::RawLanguage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_ms: Option<u64>,
    #[serde(default, flatten)]
    pub extras: Extras,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExampleBody {
    #[serde(default)]
    pub encoding: ContentEncoding,
    #[serde(default)]
    /// Decoded bytes if decode succeeded, else raw bytes (the viewer notes it).
    pub bytes: Vec<u8>,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// How the stored body bytes relate to the response content (PROMPT.md 5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ContentEncoding {
    /// Already decoded; stored as-is.
    #[default]
    Plain,
    Gzip,
    Deflate,
    Brotli,
    Zstd,
}

/// HTTP request (PROMPT.md 4.5). The URL string is the source of truth for the query string;
/// `params` and `path_vars` are the two-way views the UI maintains (PROMPT.md 5.3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequest {
    #[serde(default)]
    pub method: HttpMethod,
    #[serde(default)]
    pub url: String,
    /// Query params, two-way synced with the URL string.
    #[serde(default)]
    pub params: Vec<KeyValue>,
    /// Path variables for each `:name` segment of the URL path.
    #[serde(default)]
    pub path_vars: Vec<KeyValue>,
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub body: Body,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub scripts: Scripts,
    #[serde(default)]
    pub settings: crate::settings::RequestSettings,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// Where a gRPC request's service definition comes from (PROMPT.md 5.3.3). A new request
/// defaults to reflection until a proto set is loaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "source", rename_all = "camelCase")]
#[non_exhaustive]
pub enum ServiceDefinition {
    /// Try `grpc.reflection.v1`, then `v1alpha`, when invoking.
    #[default]
    Reflection,
    /// A named set of protos under `<workspace>/protos/<name>/`.
    ProtoSet {
        #[serde(default)]
        name: String,
        #[serde(default, flatten)]
        extras: Extras,
    },
    /// Files referenced directly by this request.
    Inline {
        #[serde(default)]
        protos: Vec<ProtoFile>,
        #[serde(default)]
        import_paths: Vec<String>,
        #[serde(default, flatten)]
        extras: Extras,
    },
}

/// One `.proto` file inside a service definition: copied into the workspace, referenced by
/// name + path (PROMPT.md 5.3.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProtoFile {
    #[serde(default)]
    pub name: String,
    /// Path under `<workspace>/protos/`.
    #[serde(default)]
    pub path: String,
    #[serde(default, flatten)]
    pub extras: Extras,
}

impl Default for WsSettings {
    fn default() -> WsSettings {
        WsSettings {
            verify_ssl: true,
            handshake_timeout_ms: 0,
            reconnect_attempts: 0,
            reconnect_interval_ms: 0,
            max_message_mb: 0,
            extras: Extras::default(),
        }
    }
}

/// gRPC request (PROMPT.md 4.5, 5.3.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrpcRequest {
    #[serde(default)]
    pub url: String,
    /// Explicit toggle only; `grpcs://` sets the UI toggle, never silently (PROMPT.md 5.3.3).
    #[serde(default)]
    pub tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_definition: Option<ServiceDefinition>,
    /// Fully qualified `Package.Service/Method` or `Service.Method`.
    #[serde(default)]
    pub method_path: String,
    /// The message JSON text the user typed (validity checked by the editor hint, not here).
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub saved_messages: Vec<SavedMessage>,
    #[serde(default)]
    pub metadata: Vec<KeyValue>,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub scripts: Scripts,
    #[serde(default)]
    pub settings: GrpcSettings,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// A named message stored on the request (grpc or ws composer save).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SavedMessage {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub text: String,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// gRPC per-request settings (PROMPT.md 5.3.3 Settings tab).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrpcSettings {
    #[serde(default = "true_value")]
    pub verify_ssl: bool,
    #[serde(default)]
    /// Overwrite server name for certificate verification (SNI override).
    pub server_name: String,
    #[serde(default)]
    pub include_default_fields: bool,
    /// 0 = unlimited (PROMPT.md 5.3.3).
    #[serde(default)]
    pub max_response_mb: u64,
    #[serde(default)]
    pub deadline_ms: u64,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// WebSocket message format options (PROMPT.md 5.3.4 composer).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "form", rename_all = "camelCase")]
#[non_exhaustive]
pub enum MessageFormat {
    #[default]
    Text,
    Json,
    Xml,
    Html,
    /// Binary payloads are stored base64 or hex encoded, per the user's choice.
    Binary {
        #[serde(default)]
        encoding: BinaryEncoding,
        #[serde(default, flatten)]
        extras: Extras,
    },
}

impl MessageFormat {
    /// The short label for the format dropdown.
    pub fn label(&self) -> &'static str {
        match self {
            MessageFormat::Text => "Text",
            MessageFormat::Json => "JSON",
            MessageFormat::Xml => "XML",
            MessageFormat::Html => "HTML",
            MessageFormat::Binary { .. } => "Binary",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum BinaryEncoding {
    #[default]
    Base64,
    Hex,
}

/// A WebSocket message saved with the request (PROMPT.md 4.5): name, format, text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WsMessage {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub format: MessageFormat,
    #[serde(default)]
    pub text: String,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// WebSocket request (PROMPT.md 4.5, 5.3.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WsRequest {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub params: Vec<KeyValue>,
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    /// Subprotocols, sent as `Sec-WebSocket-Protocol` (PROMPT.md 5.3.4).
    #[serde(default)]
    pub protocols: Vec<String>,
    #[serde(default)]
    pub saved_messages: Vec<WsMessage>,
    #[serde(default)]
    pub settings: WsSettings,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// WebSocket per-request settings (PROMPT.md 5.3.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsSettings {
    #[serde(default = "true_value")]
    pub verify_ssl: bool,
    #[serde(default)]
    pub handshake_timeout_ms: u64,
    #[serde(default)]
    pub reconnect_attempts: u32,
    #[serde(default)]
    pub reconnect_interval_ms: u64,
    /// Maximum message size in MB, 0 = unlimited (PROMPT.md 5.3.4).
    #[serde(default)]
    pub max_message_mb: u64,
    #[serde(default, flatten)]
    pub extras: Extras,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn http_request_round_trips_with_body_and_extras() {
        let request = HttpRequest {
            method: HttpMethod::Post,
            url: "https://{{host}}/v1/:id?force=1".into(),
            params: vec![KeyValue { key: "force".into(), value: "1".into(), ..Default::default() }],
            path_vars: vec![KeyValue { key: "id".into(), value: "{{idvar}}".into(), ..Default::default() }],
            headers: vec![KeyValue { key: "X-Trace".into(), value: "{{trace}}".into(), ..Default::default() }],
            body: Body::Raw { language: crate::body::RawLanguage::Json, text: "{\"a\":1}".into(), extras: Default::default() },
            ..Default::default()
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["method"], json!("POST"));
        assert_eq!(value["body"]["type"], json!("raw"));
        assert_eq!(value["body"]["language"], json!("json"));
        let back: HttpRequest = serde_json::from_value(value).unwrap();
        assert_eq!(back, request);
    }

    #[test]
    fn websocket_tls_verification_defaults_on_for_rust_and_json() {
        assert!(WsSettings::default().verify_ssl, "Rust default must verify TLS");
        let missing: WsSettings = serde_json::from_value(json!({})).unwrap();
        assert!(missing.verify_ssl, "missing field must verify TLS");
        let request = WsRequest { url: "wss://example.com".into(), ..Default::default() };
        assert!(request.settings.verify_ssl, "a new WSS request must verify TLS");
    }

    #[test]
    fn unknown_protocol_kind_does_not_lie_and_stays_untagged() {
        // A newer file's kind must not decode as a made-up HTTP request: serde must fail and
        // the importer will report it (PROMPT.md 5.7: best-effort only when RECOGNISED).
        let err = serde_json::from_value::<RequestKind>(json!({"type": "socketIO"}));
        assert!(err.is_err(), "unknown protocol kind must not silently become a variant");
    }

    #[test]
    fn grpc_request_defaults_and_service_definition_round_trip() {
        let grpc = GrpcRequest {
            url: "localhost:5051".into(),
            method_path: "pkg.Echo/Say".into(),
            service_definition: Some(ServiceDefinition::ProtoSet { name: "echo".into(), extras: Default::default() }),
            ..Default::default()
        };
        let value = serde_json::to_value(&grpc).unwrap();
        assert_eq!(value["tls"], json!(false), "TLS toggle off by default (PROMPT.md 5.3.3)");
        assert_eq!(value["serviceDefinition"]["source"], json!("protoSet"));
        let back: GrpcRequest = serde_json::from_value(value).unwrap();
        assert_eq!(back, grpc);
    }

    #[test]
    fn ws_saved_message_keeps_binary_encoding() {
        let message = WsMessage {
            name: "ping-b64".into(),
            format: MessageFormat::Binary { encoding: BinaryEncoding::Hex, extras: Default::default() },
            text: "deadbeef".into(),
            ..Default::default()
        };
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["format"]["form"], json!("binary"));
        assert_eq!(value["format"]["encoding"], json!("hex"));
    }

    #[test]
    fn scripts_is_empty_and_iterates_phases() {
        let scripts = Scripts { prerequest: Some(Script { text: "console.log(1)".into(), extras: Default::default() }), ..Default::default() };
        assert!(!scripts.is_empty());
        assert_eq!(scripts.iter().count(), 1);
        assert_eq!(Scripts::default().iter().count(), 0);
    }

    #[test]
    fn method_labels_match_the_tree_brief() {
        assert_eq!(HttpMethod::Delete.short_label(), "DEL");
        assert_eq!(HttpMethod::Options.short_label(), "OPT");
    }
}
