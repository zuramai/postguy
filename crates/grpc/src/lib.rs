//! Dynamic gRPC: compile `.proto` files with protox into a descriptor pool, encode
//! `prost-reflect::DynamicMessage` JSON payloads and call unary and streaming methods over tonic. Descriptors come from
//! compiled `.proto` files or server reflection. No codegen, no protoc.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::Path;

use prost::Message as _;
pub use prost_reflect::DescriptorPool;
use prost_reflect::{DynamicMessage, MessageDescriptor, MethodDescriptor};
use tonic::client::Grpc;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::metadata::KeyAndValueRef;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Status};

/// A gRPC failure with a user-presentable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrpcError {
    Proto(String),
    Descriptor(String),
    Json(String),
    Connect(String),
    Status { code: i32, message: String },
}

impl std::fmt::Display for GrpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrpcError::Proto(message) => write!(f, "protobuf: {message}"),
            GrpcError::Descriptor(message) => write!(f, "descriptor: {message}"),
            GrpcError::Json(message) => write!(f, "JSON: {message}"),
            GrpcError::Connect(message) => write!(f, "connection: {message}"),
            GrpcError::Status { code, message } => write!(f, "gRPC status {code}: {message}"),
        }
    }
}

impl std::error::Error for GrpcError {}

/// Compile `.proto` files (and their imports) into a descriptor pool.
pub fn compile(files: &[impl AsRef<Path>], includes: &[impl AsRef<Path>]) -> Result<DescriptorPool, GrpcError> {
    let set = compile_set(files, includes)?;
    DescriptorPool::from_file_descriptor_set(set).map_err(|err| GrpcError::Descriptor(err.to_string()))
}

/// Compile to the raw descriptor set (used by tests that serve reflection).
pub fn compile_set(files: &[impl AsRef<Path>], includes: &[impl AsRef<Path>]) -> Result<prost_types::FileDescriptorSet, GrpcError> {
    protox::compile(files, includes).map_err(|err| GrpcError::Proto(proto_message(&err)))
}

/// A compile error as `file:line:column: message` when protox knows where it happened.
fn proto_message(err: &protox::Error) -> String {
    use miette::Diagnostic as _;
    let message = err.to_string();
    let located = err.labels().and_then(|mut labels| labels.next()).and_then(|label| {
        let source = err.source_code()?;
        let contents = source.read_span(label.inner(), 0, 0).ok()?;
        let file = contents.name().map(str::to_string).or_else(|| err.file().map(str::to_string))?;
        Some(format!("{file}:{}:{}", contents.line().saturating_add(1), contents.column().saturating_add(1)))
    });
    match (located, err.file()) {
        (Some(location), _) => format!("{location}: {message}"),
        (None, Some(file)) => format!("{file}: {message}"),
        (None, None) => message,
    }
}

/// Find a method by either `package.Service/Method` or `package.Service.Method`.
pub fn method(pool: &DescriptorPool, full: &str) -> Result<MethodDescriptor, GrpcError> {
    let (service, method) = full
        .split_once('/')
        .or_else(|| full.rsplit_once('.'))
        .ok_or_else(|| GrpcError::Descriptor(format!("`{full}` must be package.Service/Method")))?;
    let service = pool.get_service_by_name(service).ok_or_else(|| GrpcError::Descriptor(format!("service `{service}` not found")))?;
    service.methods().find(|candidate| candidate.name() == method).ok_or_else(|| GrpcError::Descriptor(format!("method `{method}` not found")))
}

/// Fetch a service descriptor over gRPC reflection (v1, then v1alpha), for
/// `ServiceDefinition::Reflection`. Returns a pool containing the transitive file set the server
/// sent.
pub async fn reflection_pool(endpoint: &str, tls: &GrpcTls, method_path: &str) -> Result<DescriptorPool, GrpcError> {
    let service = method_path.split_once('/').map(|(service, _)| service).unwrap_or(method_path).trim();
    if service.is_empty() {
        return Err(GrpcError::Descriptor("reflection needs a method path so it can resolve the service".into()));
    }
    let channel = connect(endpoint, tls).await?;
    let set = match reflection_v1(&channel, service).await {
        Ok(set) => set,
        Err(GrpcError::Status { code, .. }) if code == tonic::Code::Unimplemented as i32 => reflection_v1alpha(&channel, service).await?,
        Err(error) => return Err(error),
    };
    if set.file.is_empty() {
        return Err(GrpcError::Descriptor(format!("reflection returned no files for `{service}`")));
    }
    DescriptorPool::from_file_descriptor_set(set).map_err(|err| GrpcError::Descriptor(err.to_string()))
}

async fn reflection_v1(channel: &Channel, symbol: &str) -> Result<prost_types::FileDescriptorSet, GrpcError> {
    use tonic_reflection::pb::v1 as pb;
    let mut client = pb::server_reflection_client::ServerReflectionClient::new(channel.clone());
    let request = pb::ServerReflectionRequest {
        host: String::new(),
        message_request: Some(pb::server_reflection_request::MessageRequest::FileContainingSymbol(symbol.to_string())),
    };
    let mut stream = client.server_reflection_info(Request::new(tokio_stream::iter(vec![request]))).await.map_err(status_error)?.into_inner();
    collect_reflected(&mut stream, |response| match response.message_response {
        Some(pb::server_reflection_response::MessageResponse::FileDescriptorResponse(files)) => Some(files.file_descriptor_proto),
        _ => None,
    })
    .await
}

async fn reflection_v1alpha(channel: &Channel, symbol: &str) -> Result<prost_types::FileDescriptorSet, GrpcError> {
    use tonic_reflection::pb::v1alpha as pb;
    let mut client = pb::server_reflection_client::ServerReflectionClient::new(channel.clone());
    let request = pb::ServerReflectionRequest {
        host: String::new(),
        message_request: Some(pb::server_reflection_request::MessageRequest::FileContainingSymbol(symbol.to_string())),
    };
    let mut stream = client.server_reflection_info(Request::new(tokio_stream::iter(vec![request]))).await.map_err(status_error)?.into_inner();
    collect_reflected(&mut stream, |response| match response.message_response {
        Some(pb::server_reflection_response::MessageResponse::FileDescriptorResponse(files)) => Some(files.file_descriptor_proto),
        _ => None,
    })
    .await
}

async fn collect_reflected<T, F>(stream: &mut tonic::Streaming<T>, files: F) -> Result<prost_types::FileDescriptorSet, GrpcError>
where
    F: Fn(T) -> Option<Vec<Vec<u8>>>,
{
    let mut set = prost_types::FileDescriptorSet::default();
    while let Some(response) = stream.message().await.map_err(status_error)? {
        let Some(protos) = files(response) else { continue };
        for bytes in protos {
            set.file.push(prost_types::FileDescriptorProto::decode(bytes.as_slice()).map_err(|err| GrpcError::Descriptor(err.to_string()))?);
        }
    }
    Ok(set)
}

fn status_error(status: Status) -> GrpcError {
    GrpcError::Status { code: status.code() as i32, message: status.message().to_string() }
}

/// A unary call result.
#[derive(Debug, Clone)]
pub struct UnaryResult {
    pub json: serde_json::Value,
    pub status: Option<String>,
    pub metadata: Vec<(String, String)>,
}

/// A streaming call result: every response message in arrival order (up to the cap), plus the
/// terminal metadata/status.
#[derive(Debug, Clone)]
pub struct StreamResult {
    pub messages: Vec<serde_json::Value>,
    pub status: Option<String>,
    pub metadata: Vec<(String, String)>,
}

/// Cap on collected messages, so a chatty server cannot exhaust memory (never-crash).
pub const MAX_STREAM_MESSAGES: usize = 10_000;

/// TLS options for the channel (custom CA and PEM client identity).
#[derive(Debug, Clone, Default)]
pub struct GrpcTls {
    /// `false` means plain h2c; tonic cannot express insecure verification safely.
    pub enabled: bool,
    pub ca_path: Option<String>,
    pub client_identity: Option<postguy_http::ClientIdentity>,
}

/// Call one unary method. `endpoint` is a full URL (`http://host:port` or `https://host:port`).
pub async fn unary(
    endpoint: &str,
    tls: &GrpcTls,
    method: &MethodDescriptor,
    request_json: serde_json::Value,
    metadata: &[(String, String)],
) -> Result<UnaryResult, GrpcError> {
    let input = message_from_json(method.input(), request_json)?;
    let request = request_with_metadata(input, metadata)?;
    let path = method_path(method)?;
    let channel = connect(endpoint, tls).await?;
    let mut client = Grpc::new(channel);
    client.ready().await.map_err(|err| GrpcError::Connect(err.to_string()))?;
    let response = client
        .unary(request, path, DynamicCodec::new(&method.output()))
        .await
        .map_err(|status| GrpcError::Status { code: status.code() as i32, message: status.message().to_string() })?;
    let metadata = ascii_metadata(response.metadata());
    let output = response.into_inner();
    let json = serde_json::to_value(&output).map_err(|err| GrpcError::Json(err.to_string()))?;
    Ok(UnaryResult { json, status: None, metadata })
}

/// Invoke a server-streaming method and collect up to [`MAX_STREAM_MESSAGES`] messages.
pub async fn server_streaming(
    endpoint: &str,
    tls: &GrpcTls,
    method: &MethodDescriptor,
    request_json: serde_json::Value,
    metadata: &[(String, String)],
) -> Result<StreamResult, GrpcError> {
    let input = message_from_json(method.input(), request_json)?;
    let request = request_with_metadata(input, metadata)?;
    let path = method_path(method)?;
    let channel = connect(endpoint, tls).await?;
    let mut client = Grpc::new(channel);
    client.ready().await.map_err(|err| GrpcError::Connect(err.to_string()))?;
    let response = client.server_streaming(request, path, DynamicCodec::new(&method.output())).await.map_err(status_error)?;
    collect_messages(response).await
}

/// Invoke a client-streaming method with every input message, returning the single reply.
pub async fn client_streaming(
    endpoint: &str,
    tls: &GrpcTls,
    method: &MethodDescriptor,
    messages: Vec<serde_json::Value>,
    metadata: &[(String, String)],
) -> Result<UnaryResult, GrpcError> {
    let inputs =
        messages.into_iter().take(MAX_STREAM_MESSAGES).map(|value| message_from_json(method.input(), value)).collect::<Result<Vec<_>, _>>()?;
    let request = request_with_metadata(tokio_stream::iter(inputs), metadata)?;
    let path = method_path(method)?;
    let channel = connect(endpoint, tls).await?;
    let mut client = Grpc::new(channel);
    client.ready().await.map_err(|err| GrpcError::Connect(err.to_string()))?;
    let response = client.client_streaming(request, path, DynamicCodec::new(&method.output())).await.map_err(status_error)?;
    let metadata = ascii_metadata(response.metadata());
    let output = response.into_inner();
    let json = serde_json::to_value(&output).map_err(|err| GrpcError::Json(err.to_string()))?;
    Ok(UnaryResult { json, status: None, metadata })
}

/// Invoke a bidirectional method with a finite input stream, collecting replies.
pub async fn bidi_streaming(
    endpoint: &str,
    tls: &GrpcTls,
    method: &MethodDescriptor,
    messages: Vec<serde_json::Value>,
    metadata: &[(String, String)],
) -> Result<StreamResult, GrpcError> {
    let inputs =
        messages.into_iter().take(MAX_STREAM_MESSAGES).map(|value| message_from_json(method.input(), value)).collect::<Result<Vec<_>, _>>()?;
    let request = request_with_metadata(tokio_stream::iter(inputs), metadata)?;
    let path = method_path(method)?;
    let channel = connect(endpoint, tls).await?;
    let mut client = Grpc::new(channel);
    client.ready().await.map_err(|err| GrpcError::Connect(err.to_string()))?;
    let response = client.streaming(request, path, DynamicCodec::new(&method.output())).await.map_err(status_error)?;
    collect_messages(response).await
}

async fn collect_messages(response: tonic::Response<tonic::Streaming<DynamicMessage>>) -> Result<StreamResult, GrpcError> {
    let metadata = ascii_metadata(response.metadata());
    let mut stream = response.into_inner();
    let mut messages = Vec::new();
    while let Some(message) = stream.message().await.map_err(status_error)? {
        if messages.len() >= MAX_STREAM_MESSAGES {
            return Err(GrpcError::Status {
                code: tonic::Code::ResourceExhausted as i32,
                message: format!("response exceeded {MAX_STREAM_MESSAGES} messages"),
            });
        }
        messages.push(serde_json::to_value(&message).map_err(|err| GrpcError::Json(err.to_string()))?);
    }
    Ok(StreamResult { messages, status: None, metadata })
}

fn message_from_json(descriptor: MessageDescriptor, request_json: serde_json::Value) -> Result<DynamicMessage, GrpcError> {
    let text = serde_json::to_string(&request_json).map_err(|err| GrpcError::Json(err.to_string()))?;
    DynamicMessage::deserialize(descriptor, &mut serde_json::Deserializer::from_str(&text)).map_err(|err| GrpcError::Json(err.to_string()))
}

fn request_with_metadata<T>(message: T, metadata: &[(String, String)]) -> Result<Request<T>, GrpcError> {
    use base64::Engine as _;
    let mut request = Request::new(message);
    for (name, value) in metadata {
        let name = name.to_ascii_lowercase();
        if name.ends_with("-bin") {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(value.trim())
                .map_err(|err| GrpcError::Descriptor(format!("metadata `{name}` is not base64: {err}")))?;
            let key: tonic::metadata::MetadataKey<tonic::metadata::Binary> =
                name.parse().map_err(|err| GrpcError::Descriptor(format!("metadata key `{name}`: {err}")))?;
            request.metadata_mut().append_bin(key, tonic::metadata::MetadataValue::from_bytes(&bytes));
        } else {
            let key: tonic::metadata::MetadataKey<tonic::metadata::Ascii> =
                name.parse().map_err(|err| GrpcError::Descriptor(format!("metadata key `{name}`: {err}")))?;
            let value: tonic::metadata::MetadataValue<tonic::metadata::Ascii> =
                value.parse().map_err(|err| GrpcError::Descriptor(format!("metadata value for `{name}`: {err}")))?;
            request.metadata_mut().append(key, value);
        }
    }
    Ok(request)
}

fn method_path(method: &MethodDescriptor) -> Result<http::uri::PathAndQuery, GrpcError> {
    http::uri::PathAndQuery::from_maybe_shared(format!("/{}/{}", method.parent_service().full_name(), method.name()))
        .map_err(|err| GrpcError::Descriptor(err.to_string()))
}

fn ascii_metadata(metadata: &tonic::metadata::MetadataMap) -> Vec<(String, String)> {
    metadata
        .iter()
        .filter_map(|entry| match entry {
            KeyAndValueRef::Ascii(key, value) => Some((key.as_str().to_string(), value.to_str().ok().unwrap_or_default().to_string())),
            KeyAndValueRef::Binary(key, value) => {
                use base64::Engine as _;
                Some((format!("{}-bin", key.as_str()), base64::engine::general_purpose::STANDARD.encode(value.to_bytes().ok()?)))
            }
        })
        .collect()
}

pub(crate) async fn connect(endpoint: &str, tls: &GrpcTls) -> Result<Channel, GrpcError> {
    let mut endpoint = Endpoint::from_shared(endpoint.to_string()).map_err(|err| GrpcError::Connect(err.to_string()))?;
    if tls.enabled {
        let mut config = ClientTlsConfig::new().with_native_roots();
        if let Some(path) = &tls.ca_path
            && !path.trim().is_empty()
        {
            let pem = read_pem(path).map_err(|err| GrpcError::Connect(format!("reading CA bundle {path}: {err}")))?;
            config = config.ca_certificate(tonic::transport::Certificate::from_pem(pem));
        }
        if let Some(identity) = &tls.client_identity {
            let cert = read_pem(&identity.cert_path).map_err(|err| GrpcError::Connect(format!("reading client certificate: {err}")))?;
            let key = read_pem(&identity.key_path).map_err(|err| GrpcError::Connect(format!("reading client key: {err}")))?;
            config = config.identity(tonic::transport::Identity::from_pem(cert, key));
        }
        endpoint = endpoint.tls_config(config).map_err(|err| GrpcError::Connect(err.to_string()))?;
    }
    endpoint.connect().await.map_err(|err| GrpcError::Connect(err.to_string()))
}

/// The codec's decode target: a fresh message with the method's output descriptor.
#[derive(Clone)]
pub struct DynamicCodec {
    output: MessageDescriptor,
}

impl DynamicCodec {
    pub fn new(output: &MessageDescriptor) -> DynamicCodec {
        DynamicCodec { output: output.clone() }
    }
}

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;

    fn encoder(&mut self) -> DynamicEncoder {
        DynamicEncoder
    }

    fn decoder(&mut self) -> DynamicDecoder {
        DynamicDecoder { output: self.output.clone() }
    }
}

pub struct DynamicEncoder;

impl Encoder for DynamicEncoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn encode(&mut self, item: DynamicMessage, dst: &mut EncodeBuf<'_>) -> Result<(), Status> {
        item.encode(dst).map_err(|err| Status::internal(err.to_string()))
    }
}

pub struct DynamicDecoder {
    output: MessageDescriptor,
}

impl Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<DynamicMessage>, Status> {
        let mut message = DynamicMessage::new(self.output.clone());
        message.merge(src).map_err(|err| Status::internal(err.to_string()))?;
        Ok(Some(message))
    }
}

/// Service/method inventory for the sidebar/describe tool.
pub fn describe(pool: &DescriptorPool) -> Vec<serde_json::Value> {
    let mut services: Vec<_> = pool.services().collect();
    services.sort_by_key(|service| service.full_name().to_string());
    services
        .into_iter()
        .map(|service| {
            serde_json::json!({
                "name": service.full_name(),
                "methods": service.methods().map(|method| serde_json::json!({
                    "name": method.name(),
                    "path": format!("{}/{}", service.full_name(), method.name()),
                    "input": method.input().full_name(),
                    "output": method.output().full_name(),
                    "server_streaming": method.is_server_streaming(),
                    "client_streaming": method.is_client_streaming(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// A JSON example for the method's input message: descriptor-guided values (zero primitives,
/// the first enum value, one list/map entry, the first field of each oneof), recursing with a
/// depth/cycle guard. An empty `DynamicMessage` would serialize to `{}` and hide the shape.
pub fn example_message(pool: &DescriptorPool, path: &str) -> Result<serde_json::Value, GrpcError> {
    let method = method(pool, path)?;
    let mut seen = Vec::new();
    Ok(example_for(&method.input(), &mut seen, 0))
}

fn example_for(message: &MessageDescriptor, seen: &mut Vec<String>, depth: usize) -> serde_json::Value {
    if depth >= 8 || seen.iter().any(|name| name == message.full_name()) {
        return serde_json::json!({});
    }
    seen.push(message.full_name().to_string());
    let mut object = serde_json::Map::new();
    let mut oneofs = std::collections::HashSet::new();
    for field in message.fields() {
        if let Some(oneof) = field.containing_oneof()
            && !oneofs.insert(oneof.name().to_string())
        {
            continue;
        }
        object.insert(field.json_name().to_string(), example_value(&field, seen, depth));
    }
    seen.pop();
    serde_json::Value::Object(object)
}

fn example_value(field: &prost_reflect::FieldDescriptor, seen: &mut Vec<String>, depth: usize) -> serde_json::Value {
    use prost_reflect::Kind;
    if field.is_map() {
        let Kind::Message(entry) = field.kind() else { return serde_json::json!({}) };
        let key = example_scalar(&entry.map_entry_key_field().kind(), seen, depth);
        let value = example_scalar(&entry.map_entry_value_field().kind(), seen, depth);
        let key = match key {
            serde_json::Value::String(text) => text,
            other => other.to_string(),
        };
        return serde_json::json!({ key: value });
    }
    let value = example_scalar(&field.kind(), seen, depth);
    if field.is_list() { serde_json::json!([value]) } else { value }
}

fn example_scalar(kind: &prost_reflect::Kind, seen: &mut Vec<String>, depth: usize) -> serde_json::Value {
    use prost_reflect::Kind;
    match kind {
        Kind::Double | Kind::Float => serde_json::json!(0.0),
        Kind::Int32
        | Kind::Int64
        | Kind::Sint32
        | Kind::Sint64
        | Kind::Sfixed32
        | Kind::Sfixed64
        | Kind::Uint32
        | Kind::Uint64
        | Kind::Fixed32
        | Kind::Fixed64 => serde_json::json!(0),
        Kind::Bool => serde_json::json!(false),
        Kind::String | Kind::Bytes => serde_json::json!(""),
        Kind::Enum(descriptor) => {
            descriptor.values().next().map(|value| serde_json::Value::String(value.name().to_string())).unwrap_or(serde_json::Value::Null)
        }
        Kind::Message(descriptor) => example_wkt_or_message(descriptor, seen, depth),
    }
}

fn example_wkt_or_message(message: &MessageDescriptor, seen: &mut Vec<String>, depth: usize) -> serde_json::Value {
    match message.full_name() {
        "google.protobuf.Timestamp" => serde_json::json!("1970-01-01T00:00:00Z"),
        "google.protobuf.Duration" => serde_json::json!("0s"),
        "google.protobuf.FieldMask" => serde_json::json!(""),
        "google.protobuf.Empty" | "google.protobuf.Struct" => serde_json::json!({}),
        "google.protobuf.Value" => serde_json::Value::Null,
        "google.protobuf.ListValue" => serde_json::json!([]),
        "google.protobuf.Any" => serde_json::json!({"@type": "type.googleapis.com/google.protobuf.Empty"}),
        "google.protobuf.BoolValue" => serde_json::json!(false),
        "google.protobuf.StringValue" | "google.protobuf.BytesValue" => serde_json::json!(""),
        "google.protobuf.Int32Value" | "google.protobuf.Int64Value" | "google.protobuf.UInt32Value" | "google.protobuf.UInt64Value" => {
            serde_json::json!(0)
        }
        "google.protobuf.FloatValue" | "google.protobuf.DoubleValue" => serde_json::json!(0.0),
        _ => example_for(message, seen, depth + 1),
    }
}

/// Byte/file caps for a confined proto compile (never-crash: imported files are untrusted).
pub const MAX_PROTO_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_PROTO_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_PROTO_FILES: usize = 256;

/// Compile `files` (names relative to one of `roots`) with every resolved file confined to its
/// canonical root. Symlinks and `..` imports that escape a root are refused; per-file and
/// aggregate byte budgets plus a file count cap bound expansion. Google well-known types are
/// served by protox's built-in resolver.
pub fn compile_confined(roots: &[std::path::PathBuf], files: &[String]) -> Result<DescriptorPool, GrpcError> {
    use protox::file::{ChainFileResolver, GoogleFileResolver};

    let mut canonical_roots = Vec::new();
    for root in roots {
        let canonical = root.canonicalize().map_err(|err| GrpcError::Proto(format!("include path {}: {err}", root.display())))?;
        if !canonical.is_dir() {
            return Err(GrpcError::Proto(format!("include path {} is not a directory", root.display())));
        }
        if !canonical_roots.contains(&canonical) {
            canonical_roots.push(canonical);
        }
    }
    if canonical_roots.is_empty() {
        return Err(GrpcError::Proto("no include roots for the proto files".into()));
    }
    let mut chain = ChainFileResolver::new();
    chain.add(ConfinedResolver::new(canonical_roots.clone()));
    chain.add(GoogleFileResolver::new());
    let mut compiler = protox::Compiler::with_file_resolver(chain);
    compiler.include_imports(true);
    for file in files {
        compiler.open_file(file).map_err(|err| GrpcError::Proto(proto_message(&err)))?;
    }
    let set = compiler.file_descriptor_set();
    DescriptorPool::from_file_descriptor_set(set).map_err(|err| GrpcError::Descriptor(err.to_string()))
}

struct ConfinedResolver {
    roots: Vec<std::path::PathBuf>,
    stats: std::sync::Mutex<ResolverStats>,
}

struct ResolverStats {
    files: usize,
    bytes: u64,
}

impl ConfinedResolver {
    fn new(roots: Vec<std::path::PathBuf>) -> ConfinedResolver {
        ConfinedResolver { roots, stats: std::sync::Mutex::new(ResolverStats { files: 0, bytes: 0 }) }
    }

    fn open(&self, name: &str) -> Result<protox::file::File, protox::Error> {
        if name.is_empty() || std::path::Path::new(name).is_absolute() {
            return Err(protox::Error::file_not_found(name));
        }
        if std::path::Path::new(name)
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)))
        {
            return Err(protox::Error::file_not_found(name));
        }
        for root in &self.roots {
            let candidate = root.join(name);
            let Ok(canonical) = candidate.canonicalize() else { continue };
            if !canonical.starts_with(root) || !canonical.is_file() {
                continue;
            }
            let Ok(metadata) = std::fs::metadata(&canonical) else { continue };
            if metadata.len() > MAX_PROTO_FILE_BYTES {
                return Err(protox::Error::file_not_found(name));
            }
            let mut stats = self.stats.lock().map_err(|_| protox::Error::file_not_found(name))?;
            if stats.files >= MAX_PROTO_FILES || stats.bytes.saturating_add(metadata.len()) > MAX_PROTO_TOTAL_BYTES {
                return Err(protox::Error::file_not_found(name));
            }
            let mut source = String::new();
            let read = std::fs::File::open(&canonical).and_then(|file| {
                use std::io::Read as _;
                file.take(MAX_PROTO_FILE_BYTES + 1).read_to_string(&mut source)
            });
            let Ok(read) = read else { continue };
            if read as u64 > MAX_PROTO_FILE_BYTES {
                return Err(protox::Error::file_not_found(name));
            }
            stats.files += 1;
            stats.bytes = stats.bytes.saturating_add(read as u64);
            return protox::file::File::from_source(name, &source);
        }
        Err(protox::Error::file_not_found(name))
    }
}

impl protox::file::FileResolver for ConfinedResolver {
    fn open_file(&self, name: &str) -> Result<protox::file::File, protox::Error> {
        self.open(name)
    }
}

/// Read a PEM file with a hard cap (certificates are user-chosen paths; a huge file must not
/// allocate unbounded memory before the TLS stack rejects it).
fn read_pem(path: &str) -> Result<Vec<u8>, String> {
    const MAX_PEM_BYTES: u64 = 1024 * 1024;
    let metadata = std::fs::metadata(path).map_err(|err| err.to_string())?;
    if metadata.len() > MAX_PEM_BYTES {
        return Err(format!("file is larger than {MAX_PEM_BYTES} bytes"));
    }
    let file = std::fs::File::open(path).map_err(|err| err.to_string())?;
    let mut bytes = Vec::new();
    use std::io::Read as _;
    file.take(MAX_PEM_BYTES + 1).read_to_end(&mut bytes).map_err(|err| err.to_string())?;
    if bytes.len() as u64 > MAX_PEM_BYTES {
        return Err(format!("file is larger than {MAX_PEM_BYTES} bytes"));
    }
    Ok(bytes)
}
