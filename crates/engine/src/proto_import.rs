//! `.proto` import: copy the picked files (and the imports they pull in) into a workspace
//! proto set under `protos/<name>/`, compile the set to prove it works, and create a collection
//! with one gRPC request per service method.

use std::panic::AssertUnwindSafe;
use std::path::{Component, Path, PathBuf};

use postguy_grpc::{MAX_PROTO_FILE_BYTES, MAX_PROTO_FILES};
use postguy_model::{Collection, Folder, GrpcRequest, Id, Item, Request, RequestKind, ServiceDefinition, Variable};
use serde::Serialize;

use crate::Engine;
use crate::error::{EngineError, Result};
use crate::grpc::{confined_join, relative_name};
use crate::import::{ImportOptions, ImportOutcome};

/// What to call the set and how to treat a name that is already taken.
#[derive(Debug, Clone, Default)]
pub struct ProtoImportOptions {
    /// The proto set (and collection) name; default: the first file's stem.
    pub name: Option<String>,
    /// Move an existing set of the same name to the trash instead of suffixing the new one.
    pub replace: bool,
}

/// One completed `.proto` import.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtoImportOutcome {
    /// The proto set under `protos/`.
    pub set: String,
    /// Files copied into the set, relative to it.
    pub files: Vec<String>,
    pub collection: ImportOutcome,
    pub services: usize,
    pub methods: usize,
}

fn import_error(message: impl Into<String>) -> EngineError {
    EngineError::Import { message: message.into() }
}

/// The deepest directory that contains every file; `None` when they share only the filesystem
/// root (or nothing).
fn common_root(files: &[PathBuf]) -> Option<PathBuf> {
    let mut parents = files.iter().filter_map(|file| file.parent());
    let mut prefix: Vec<Component<'_>> = parents.next()?.components().collect();
    for parent in parents {
        let components: Vec<Component<'_>> = parent.components().collect();
        let shared = prefix.iter().zip(&components).take_while(|(a, b)| a == b).count();
        prefix.truncate(shared);
    }
    let root: PathBuf = prefix.into_iter().collect();
    root.parent().is_some().then_some(root)
}

/// `[A-Za-z0-9_-]` only, other characters become `-`; never empty.
fn sanitize(name: &str) -> String {
    let mapped: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect();
    let trimmed = mapped.trim_matches('-');
    if trimmed.is_empty() { "protos".to_string() } else { trimmed.to_string() }
}

/// `base`, or `base-2` .. `base-1000`, whichever does not exist under `protos_dir` yet.
fn free_set_name(protos_dir: &Path, base: &str) -> Result<String> {
    if !protos_dir.join(base).exists() {
        return Ok(base.to_string());
    }
    (2..=1000)
        .map(|suffix| format!("{base}-{suffix}"))
        .find(|candidate| !protos_dir.join(candidate).exists())
        .ok_or_else(|| import_error(format!("there are already 1000 proto sets named `{base}`; pass a different name")))
}

impl Engine {
    /// Import `.proto` files as a proto set plus a gRPC collection. An internal panic becomes an
    /// error, never a crash.
    pub fn import_protos(&self, files: &[PathBuf], options: ProtoImportOptions) -> Result<ProtoImportOutcome> {
        match std::panic::catch_unwind(AssertUnwindSafe(|| self.import_protos_inner(files, options))) {
            Ok(result) => result,
            Err(payload) => {
                let message = payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_string());
                Err(import_error(format!("the importer failed unexpectedly: {message}")))
            }
        }
    }

    fn import_protos_inner(&self, files: &[PathBuf], options: ProtoImportOptions) -> Result<ProtoImportOutcome> {
        if files.is_empty() || files.len() > MAX_PROTO_FILES {
            return Err(import_error("choose one or more .proto files"));
        }
        let mut canonical = Vec::with_capacity(files.len());
        for file in files {
            let is_proto = file.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("proto"));
            let metadata = std::fs::symlink_metadata(file).map_err(|err| import_error(format!("{}: {err}", file.display())))?;
            if !is_proto || !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(import_error("choose one or more .proto files"));
            }
            canonical.push(file.canonicalize().map_err(|err| import_error(format!("{}: {err}", file.display())))?);
        }
        let Some(root) = common_root(&canonical) else {
            return Err(import_error("the .proto files must be in one folder tree (not directly under the filesystem root)"));
        };
        let mut names = Vec::with_capacity(canonical.len());
        for file in &canonical {
            names.push(relative_name(&root, file)?);
        }
        let pool = postguy_grpc::compile_confined(std::slice::from_ref(&root), &names)
            .map_err(|err| import_error(format!("the proto files do not compile: {err}")))?;

        // Every file the picked ones pulled in, except the bundled google/protobuf ones.
        let mut copy: Vec<String> = pool.files().map(|file| file.name().to_string()).filter(|name| !name.starts_with("google/")).collect();
        copy.sort();
        copy.dedup();

        let base = sanitize(
            &options
                .name
                .clone()
                .or_else(|| canonical.first().and_then(|file| file.file_stem()).map(|stem| stem.to_string_lossy().into_owned()))
                .unwrap_or_default(),
        );
        let protos_dir = self.store_paths().protos_dir();
        std::fs::create_dir_all(&protos_dir).map_err(|err| import_error(format!("creating {}: {err}", protos_dir.display())))?;
        let set = if options.replace {
            let existing = protos_dir.join(&base);
            if existing.exists() {
                postguy_store::trash::move_to_trash(self.store_paths(), &existing, "proto set")?;
            }
            base
        } else {
            free_set_name(&protos_dir, &base)?
        };
        let set_dir = protos_dir.join(&set);

        let result = self.copy_and_build(&root, &copy, &set, &set_dir);
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&set_dir);
        }
        result
    }

    /// Copy the files, prove the set compiles through the normal `ProtoSet` path, build and save
    /// the collection.
    fn copy_and_build(&self, root: &Path, copy: &[String], set: &str, set_dir: &Path) -> Result<ProtoImportOutcome> {
        for name in copy {
            let source = confined_join(root, name)?;
            let bytes = postguy_util::read_file_capped(&source, usize::try_from(MAX_PROTO_FILE_BYTES).unwrap_or(usize::MAX))
                .map_err(|err| import_error(format!("{}: {err}", source.display())))?;
            postguy_util::atomic_write(&set_dir.join(name), &bytes).map_err(|err| import_error(format!("writing {name}: {err}")))?;
        }
        let definition = ServiceDefinition::ProtoSet { name: set.to_string(), extras: Default::default() };
        let pool = self.grpc_pool(&GrpcRequest { service_definition: Some(definition.clone()), ..GrpcRequest::default() })?;

        let mut folders = Vec::new();
        let mut methods = 0usize;
        for service in postguy_grpc::describe(&pool) {
            let service_name = service["name"].as_str().unwrap_or_default().to_string();
            let mut items = Vec::new();
            for method in service["methods"].as_array().into_iter().flatten() {
                let (Some(name), Some(path)) = (method["name"].as_str(), method["path"].as_str()) else { continue };
                let message = postguy_grpc::example_message(&pool, path)
                    .ok()
                    .and_then(|example| serde_json::to_string_pretty(&example).ok())
                    .unwrap_or_else(|| "{}".to_string());
                let request = GrpcRequest {
                    url: "{{grpcUrl}}".into(),
                    tls: false,
                    service_definition: Some(definition.clone()),
                    method_path: path.to_string(),
                    message,
                    ..GrpcRequest::default()
                };
                items.push(Item::Request(Box::new(Request {
                    id: Id::generate(),
                    name: name.to_string(),
                    kind: RequestKind::Grpc(request),
                    ..Request::default()
                })));
                methods += 1;
            }
            if !items.is_empty() {
                folders.push(Item::Folder(Box::new(Folder { id: Id::generate(), name: service_name, items, ..Folder::default() })));
            }
        }
        if methods == 0 {
            return Err(import_error("the .proto files define no services, so there is nothing to call"));
        }
        let services = folders.len();
        let collection = Collection {
            id: Id::generate(),
            name: set.to_string(),
            variables: vec![Variable { key: "grpcUrl".into(), value: "localhost:50051".into(), ..Variable::default() }],
            items: folders,
            ..Collection::default()
        };
        let report = postguy_interop::ImportReport { collections: 1, folders: services, requests: methods, ..Default::default() };
        let collection = self.save_imported_collection(collection, report, ImportOptions::default())?;
        Ok(ProtoImportOutcome { set: set.to_string(), files: copy.to_vec(), collection, services, methods })
    }
}
