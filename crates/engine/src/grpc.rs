//! gRPC descriptors and unary invocation (M7.1/M7.2): compile a request's proto set/inline
//! files from the workspace, then call one method with the dynamic codec. Reflection and
//! streaming are later milestones.

use std::path::PathBuf;

use postguy_grpc::{DescriptorPool, GrpcError, GrpcTls, StreamResult, UnaryResult};
use postguy_model::{GrpcRequest, ServiceDefinition};

use crate::Engine;
use crate::error::{EngineError, Result};

impl Engine {
    /// Compile the protos referenced by a gRPC request.
    pub fn grpc_pool(&self, request: &GrpcRequest) -> Result<DescriptorPool> {
        let protos = self.store_paths().protos_dir();
        match request.service_definition.as_ref().unwrap_or(&ServiceDefinition::Reflection) {
            ServiceDefinition::Reflection => {
                let (endpoint, tls) = self.grpc_connection(request)?;
                self.inner.runtime.block_on(postguy_grpc::reflection_pool(&endpoint, &tls, &request.method_path)).map_err(grpc_error)
            }
            ServiceDefinition::ProtoSet { name, .. } => {
                if name.trim().is_empty() {
                    return Err(EngineError::BadRequest { reason: "the proto set has no name".into() });
                }
                let dir = protos.join(name);
                let files = proto_files(&dir)?;
                if files.is_empty() {
                    return Err(EngineError::BadRequest { reason: format!("no .proto files under {}", dir.display()) });
                }
                postguy_grpc::compile(&files, &[dir]).map_err(grpc_error)
            }
            ServiceDefinition::Inline { protos: files, import_paths, .. } => {
                let paths: Vec<PathBuf> = files.iter().filter(|file| !file.path.trim().is_empty()).map(|file| protos.join(&file.path)).collect();
                if paths.is_empty() {
                    return Err(EngineError::BadRequest { reason: "the inline service definition has no proto files".into() });
                }
                let mut includes: Vec<PathBuf> = vec![protos.clone()];
                includes.extend(import_paths.iter().filter(|path| !path.trim().is_empty()).map(|path| protos.join(path)));
                postguy_grpc::compile(&paths, &includes).map_err(grpc_error)
            }
            _ => Err(EngineError::BadRequest { reason: "unknown service definition shape; update Postguy".into() }),
        }
    }

    /// Service/method inventory for a request's compiled protos.
    pub fn grpc_describe(&self, request: &GrpcRequest) -> Result<Vec<serde_json::Value>> {
        let pool = self.grpc_pool(request)?;
        Ok(postguy_grpc::describe(&pool))
    }

    /// A default JSON example for the request's input message.
    pub fn grpc_example(&self, request: &GrpcRequest) -> Result<serde_json::Value> {
        let pool = self.grpc_pool(request)?;
        postguy_grpc::example_message(&pool, &request.method_path).map_err(grpc_error)
    }

    /// Invoke a server-, client- or bidi-streaming method. A unary method is rejected; a
    /// client-streaming reply becomes the single element of `messages`.
    pub fn grpc_stream(&self, request: &GrpcRequest, messages: Vec<serde_json::Value>) -> Result<StreamResult> {
        let pool = self.grpc_pool(request)?;
        let method = postguy_grpc::method(&pool, &request.method_path).map_err(grpc_error)?;
        let (endpoint, tls) = self.grpc_connection(request)?;
        let metadata: Vec<(String, String)> =
            request.metadata.iter().filter(|row| row.enabled && !row.key.is_empty()).map(|row| (row.key.clone(), row.value.clone())).collect();
        let runtime = &self.inner.runtime;
        if method.is_server_streaming() && !method.is_client_streaming() {
            let first = messages
                .into_iter()
                .next()
                .ok_or_else(|| EngineError::BadRequest { reason: "a server-streaming call needs one request message".into() })?;
            runtime.block_on(postguy_grpc::server_streaming(&endpoint, &tls, &method, first, &metadata)).map_err(grpc_error)
        } else if method.is_client_streaming() && !method.is_server_streaming() {
            let result = runtime.block_on(postguy_grpc::client_streaming(&endpoint, &tls, &method, messages, &metadata)).map_err(grpc_error)?;
            Ok(StreamResult { messages: vec![result.json], status: result.status, metadata: result.metadata })
        } else if method.is_client_streaming() && method.is_server_streaming() {
            runtime.block_on(postguy_grpc::bidi_streaming(&endpoint, &tls, &method, messages, &metadata)).map_err(grpc_error)
        } else {
            Err(EngineError::BadRequest { reason: "this method is unary; use grpc_invoke".into() })
        }
    }

    /// Invoke one unary method with JSON input. Blocks via the engine runtime.
    pub fn grpc_invoke_unary(&self, request: &GrpcRequest, request_json: serde_json::Value) -> Result<UnaryResult> {
        let pool = self.grpc_pool(request)?;
        let method = postguy_grpc::method(&pool, &request.method_path).map_err(grpc_error)?;
        let (endpoint, tls) = self.grpc_connection(request)?;
        let metadata: Vec<(String, String)> =
            request.metadata.iter().filter(|row| row.enabled && !row.key.is_empty()).map(|row| (row.key.clone(), row.value.clone())).collect();
        self.inner.runtime.block_on(postguy_grpc::unary(&endpoint, &tls, &method, request_json, &metadata)).map_err(grpc_error)
    }
}

impl Engine {
    fn grpc_connection(&self, request: &GrpcRequest) -> Result<(String, GrpcTls)> {
        if request.url.trim().is_empty() {
            return Err(EngineError::BadRequest { reason: "the gRPC endpoint URL is empty".into() });
        }
        let scheme = if request.tls { "https" } else { "http" };
        let endpoint = if request.url.contains("://") { request.url.clone() } else { format!("{scheme}://{}", request.url.trim_start_matches("//")) };
        let global = self.settings();
        let tls = GrpcTls {
            enabled: request.tls,
            ca_path: (!global.ca_bundle_path.trim().is_empty()).then(|| global.ca_bundle_path.clone()),
            client_identity: if !global.client_cert_path.trim().is_empty() && !global.client_key_path.trim().is_empty() {
                Some(postguy_http::ClientIdentity { cert_path: global.client_cert_path.clone(), key_path: global.client_key_path.clone() })
            } else {
                None
            },
        };
        Ok((endpoint, tls))
    }
}

fn proto_files(dir: &std::path::Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|err| EngineError::BadRequest { reason: format!("reading {}: {err}", dir.display()) })?;
    for entry in entries {
        let entry = entry.map_err(|err| EngineError::BadRequest { reason: format!("reading {}: {err}", dir.display()) })?;
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("proto")) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn grpc_error(error: GrpcError) -> EngineError {
    EngineError::BadRequest { reason: error.to_string() }
}
