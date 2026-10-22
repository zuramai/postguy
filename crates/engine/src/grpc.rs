//! gRPC descriptors and unary invocation: compile a request's proto set/inline
//! files from the workspace, then call one method with the dynamic codec. Reflection-based
//! descriptors and streaming calls sit alongside unary.

use std::path::PathBuf;

use postguy_grpc::{DescriptorPool, GrpcError, GrpcTls, StreamResult, UnaryResult};
use postguy_model::{Collection, Folder, GrpcRequest, Item, KeyValue, Request, ServiceDefinition};

use crate::Engine;
use crate::error::{EngineError, Result};
use crate::pipeline::{Located, build_resolver, resolve_auth, resolved_auth_chain};

impl Engine {
    /// Compile the protos referenced by a gRPC request.
    /// Resolve a stored/draft gRPC request the way an HTTP send would: collection, folder and
    /// environment variables (including secrets) in every text field, then inherited auth
    /// applied as metadata. Unresolved variables and unsupported auth fail instead of being
    /// sent literally.
    pub fn prepare_grpc(&self, collection: &postguy_model::Id, request: &postguy_model::Id, draft: &GrpcRequest) -> Result<GrpcRequest> {
        let (collection, folders, stored) = self.locate_grpc(collection, request)?;
        let located = Located { collection, folders, http: postguy_model::HttpRequest { auth: stored.auth.clone(), ..Default::default() } };
        let resolver = build_resolver(self, &located, None)?;
        let mut prepared = draft.clone();
        prepared.url = resolve_checked(&resolver, &draft.url, "URL")?;
        prepared.method_path = resolve_checked(&resolver, &draft.method_path, "method path")?;
        prepared.message = resolve_checked(&resolver, &draft.message, "request message")?;
        for row in &mut prepared.metadata {
            row.key = resolve_checked(&resolver, &row.key, "metadata key")?;
            row.value = resolve_checked(&resolver, &row.value, "metadata value")?;
        }
        if let Some(definition) = prepared.service_definition.as_mut() {
            match definition {
                ServiceDefinition::ProtoSet { name, .. } => *name = resolve_checked(&resolver, name, "proto set name")?,
                ServiceDefinition::Inline { protos, import_paths, .. } => {
                    for file in protos {
                        file.name = resolve_checked(&resolver, &file.name, "proto file name")?;
                        file.path = resolve_checked(&resolver, &file.path, "proto file path")?;
                    }
                    for path in import_paths {
                        *path = resolve_checked(&resolver, path, "import path")?;
                    }
                }
                _ => {}
            }
        }
        let auth = resolve_auth(&resolved_auth_chain(&located), &resolver)?;
        if let Some(auth) = auth {
            let mut spec = postguy_http::RequestSpec::get(prepared.url.clone());
            let placement = postguy_http::apply_auth(&auth, &mut spec).map_err(|err| EngineError::BadRequest { reason: err.to_string() })?;
            if matches!(placement, postguy_http::AuthPlacement::Query) {
                return Err(EngineError::BadRequest { reason: "API-key query auth is not supported for gRPC; use the header location".into() });
            }
            for (key, value) in spec.headers {
                prepared.metadata.retain(|row| !row.key.eq_ignore_ascii_case(&key));
                prepared.metadata.push(KeyValue { key, value, enabled: true, ..Default::default() });
            }
        }
        Ok(prepared)
    }

    fn locate_grpc(&self, collection_id: &postguy_model::Id, request_id: &postguy_model::Id) -> Result<(Collection, Vec<Folder>, GrpcRequest)> {
        let collection = self.workspace().load_collection(collection_id)?.into_value();
        let mut stack: Vec<(Vec<Folder>, Vec<Item>)> = vec![(Vec::new(), collection.items.clone())];
        let mut visited = 0usize;
        while let Some((folders, items)) = stack.pop() {
            visited = visited.saturating_add(items.len());
            if visited > 200_000 {
                return Err(EngineError::BadRequest { reason: "the collection tree is too large to search".into() });
            }
            for item in items {
                match item {
                    Item::Folder(folder) => {
                        let mut child = folders.clone();
                        child.push((*folder).clone());
                        stack.push((child, folder.items.clone()));
                    }
                    Item::Request(found) if found.id == *request_id => {
                        let Request { kind, .. } = *found;
                        let postguy_model::RequestKind::Grpc(grpc) = kind else {
                            return Err(EngineError::BadRequest { reason: "prepare_grpc expects a gRPC request".into() });
                        };
                        return Ok((collection, folders, grpc));
                    }
                    Item::Request(_) => {}
                }
            }
        }
        Err(EngineError::NotFound { what: "request".into(), id: request_id.0.clone() })
    }

    pub fn grpc_pool(&self, request: &GrpcRequest) -> Result<DescriptorPool> {
        let protos = self.store_paths().protos_dir();
        let root = protos
            .canonicalize()
            .map_err(|err| EngineError::BadRequest { reason: format!("workspace protos directory {}: {err}", protos.display()) })?;
        match request.service_definition.as_ref().unwrap_or(&ServiceDefinition::Reflection) {
            ServiceDefinition::Reflection => {
                let (endpoint, tls) = self.grpc_connection(request)?;
                self.inner.runtime.block_on(postguy_grpc::reflection_pool(&endpoint, &tls, &request.method_path)).map_err(grpc_error)
            }
            ServiceDefinition::ProtoSet { name, .. } => {
                if name.is_empty() {
                    return Err(EngineError::BadRequest { reason: "the proto set has no name".into() });
                }
                let set_dir = confined_join(&root, name)?;
                if !set_dir.is_dir() {
                    return Err(EngineError::BadRequest { reason: format!("proto set `{name}` is not a directory under the workspace protos root") });
                }
                let mut files = Vec::new();
                collect_proto_files(&root, &set_dir, 0, &mut files)?;
                if files.is_empty() {
                    return Err(EngineError::BadRequest { reason: format!("no .proto files under proto set `{name}`") });
                }
                postguy_grpc::compile_confined(std::slice::from_ref(&root), &files).map_err(grpc_error)
            }
            ServiceDefinition::Inline { protos: files, import_paths, .. } => {
                let mut names = Vec::new();
                for file in files.iter().filter(|file| !file.path.trim().is_empty()) {
                    let path = confined_join(&root, &file.path)?;
                    if !path.is_file() {
                        return Err(EngineError::BadRequest {
                            reason: format!("inline proto `{}` is not a file under the workspace protos root", file.path),
                        });
                    }
                    names.push(relative_name(&root, &path)?);
                }
                if names.is_empty() {
                    return Err(EngineError::BadRequest { reason: "the inline service definition has no proto files".into() });
                }
                let mut roots = vec![root.clone()];
                for import in import_paths.iter().filter(|path| !path.trim().is_empty()) {
                    let dir = confined_join(&root, import)?;
                    if !dir.is_dir() {
                        return Err(EngineError::BadRequest {
                            reason: format!("import path `{import}` is not a directory under the workspace protos root"),
                        });
                    }
                    roots.push(dir);
                }
                postguy_grpc::compile_confined(&roots, &names).map_err(grpc_error)
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

/// Join one untrusted component under a canonical root, refusing absolute paths, `..` and
/// symlinks that resolve outside the root. Nonexistent paths refuse the same way.
fn confined_join(root: &std::path::Path, untrusted: &str) -> Result<PathBuf> {
    let relative = std::path::Path::new(untrusted);
    if untrusted.trim().is_empty() || relative.is_absolute() {
        return Err(EngineError::BadRequest { reason: format!("path `{untrusted}` must be relative to the workspace protos root") });
    }
    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)))
    {
        return Err(EngineError::BadRequest { reason: format!("path `{untrusted}` may not leave the workspace protos root") });
    }
    let joined = root.join(relative);
    let canonical = joined.canonicalize().map_err(|err| EngineError::BadRequest { reason: format!("path `{untrusted}`: {err}") })?;
    if !canonical.starts_with(root) {
        return Err(EngineError::BadRequest { reason: format!("path `{untrusted}` may not leave the workspace protos root") });
    }
    Ok(canonical)
}

fn relative_name(root: &std::path::Path, path: &std::path::Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| EngineError::BadRequest { reason: format!("path `{}` is outside the workspace protos root", path.display()) })?;
    let mut name = String::new();
    for component in relative.components() {
        let std::path::Component::Normal(part) = component else {
            return Err(EngineError::BadRequest { reason: format!("path `{}` is outside the workspace protos root", path.display()) });
        };
        let Some(part) = part.to_str() else {
            return Err(EngineError::BadRequest { reason: format!("path `{}` is not UTF-8", path.display()) });
        };
        if !name.is_empty() {
            name.push('/');
        }
        name.push_str(part);
    }
    Ok(name)
}

fn collect_proto_files(root: &std::path::Path, dir: &std::path::Path, depth: usize, files: &mut Vec<String>) -> Result<()> {
    const MAX_DEPTH: usize = 8;
    const MAX_FILES: usize = 256;
    if depth > MAX_DEPTH {
        return Err(EngineError::BadRequest { reason: "proto set nesting is too deep".into() });
    }
    if files.len() >= MAX_FILES {
        return Err(EngineError::BadRequest { reason: format!("proto set has more than {MAX_FILES} files") });
    }
    let entries = std::fs::read_dir(dir).map_err(|err| EngineError::BadRequest { reason: format!("reading {}: {err}", dir.display()) })?;
    for entry in entries {
        let entry = entry.map_err(|err| EngineError::BadRequest { reason: format!("reading {}: {err}", dir.display()) })?;
        let path = entry.path();
        let canonical = path.canonicalize().map_err(|err| EngineError::BadRequest { reason: format!("path {}: {err}", path.display()) })?;
        if !canonical.starts_with(root) {
            return Err(EngineError::BadRequest { reason: format!("path {} leaves the workspace protos root", path.display()) });
        }
        if canonical.is_dir() {
            collect_proto_files(root, &canonical, depth + 1, files)?;
        } else if canonical.is_file() && canonical.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("proto")) {
            files.push(relative_name(root, &canonical)?);
            if files.len() > MAX_FILES {
                return Err(EngineError::BadRequest { reason: format!("proto set has more than {MAX_FILES} files") });
            }
        }
    }
    files.sort();
    Ok(())
}

fn grpc_error(error: GrpcError) -> EngineError {
    EngineError::BadRequest { reason: error.to_string() }
}

fn resolve_checked(resolver: &postguy_vars::Resolver, text: &str, field: &str) -> Result<String> {
    let resolved = resolver.resolve_text(text);
    let unresolved = resolved.unresolved_names();
    if !unresolved.is_empty() {
        return Err(EngineError::BadRequest { reason: format!("the gRPC {field} has unresolved variable(s): {}", unresolved.join(", ")) });
    }
    Ok(resolved.text)
}
