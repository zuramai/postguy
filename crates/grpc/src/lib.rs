//! Dynamic gRPC (M7.1/M7.2): compile `.proto` files with protox into a descriptor pool, encode
//! `prost-reflect::DynamicMessage` JSON payloads and call unary methods over tonic. No codegen,
//! no protoc. Streaming and reflection arrive in M7.3/M7.4.

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
    protox::compile(files, includes).map_err(|err| GrpcError::Proto(err.to_string()))
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
    let channel = connect(endpoint, tls).await?;
    let json = serde_json::to_string(&request_json).map_err(|err| GrpcError::Json(err.to_string()))?;
    let input = DynamicMessage::deserialize(method.input(), &mut serde_json::Deserializer::from_str(&json))
        .map_err(|err| GrpcError::Json(err.to_string()))?;
    let mut request = Request::new(input);
    for (name, value) in metadata {
        if let Ok(name) = name.parse::<tonic::metadata::MetadataKey<tonic::metadata::Ascii>>()
            && let Ok(value) = value.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
        {
            request.metadata_mut().append(name, value);
        }
    }
    let path = http::uri::PathAndQuery::from_maybe_shared(format!("/{}/{}", method.parent_service().full_name(), method.name()))
        .map_err(|err| GrpcError::Descriptor(err.to_string()))?;
    let mut client = Grpc::new(channel);
    client.ready().await.map_err(|err| GrpcError::Connect(err.to_string()))?;
    let response = client
        .unary(request, path, DynamicCodec::new(&method.output()))
        .await
        .map_err(|status| GrpcError::Status { code: status.code() as i32, message: status.message().to_string() })?;
    let metadata = response
        .metadata()
        .iter()
        .filter_map(|entry| match entry {
            KeyAndValueRef::Ascii(key, value) => Some((key.as_str().to_string(), value.to_str().ok().unwrap_or_default().to_string())),
            KeyAndValueRef::Binary(_, _) => None,
        })
        .collect();
    let output = response.into_inner();
    let json = serde_json::to_value(&output).map_err(|err| GrpcError::Json(err.to_string()))?;
    Ok(UnaryResult { json, status: None, metadata })
}

pub(crate) async fn connect(endpoint: &str, tls: &GrpcTls) -> Result<Channel, GrpcError> {
    let mut endpoint = Endpoint::from_shared(endpoint.to_string()).map_err(|err| GrpcError::Connect(err.to_string()))?;
    if tls.enabled {
        let mut config = ClientTlsConfig::new();
        if let Some(path) = &tls.ca_path
            && !path.trim().is_empty()
        {
            let pem = std::fs::read(path).map_err(|err| GrpcError::Connect(format!("reading CA bundle {path}: {err}")))?;
            config = config.ca_certificate(tonic::transport::Certificate::from_pem(pem));
        }
        if let Some(identity) = &tls.client_identity {
            let cert = std::fs::read(&identity.cert_path).map_err(|err| GrpcError::Connect(err.to_string()))?;
            let key = std::fs::read(&identity.key_path).map_err(|err| GrpcError::Connect(err.to_string()))?;
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

/// A JSON example with default values for the method's input message.
pub fn example_message(pool: &DescriptorPool, path: &str) -> Result<serde_json::Value, GrpcError> {
    let method = method(pool, path)?;
    let message = DynamicMessage::new(method.input());
    serde_json::to_value(&message).map_err(|err| GrpcError::Json(err.to_string()))
}
