//! The engine façade (L6, PROMPT.md 4.1): one session owning the workspace handle, the runtime,
//! the settings snapshot, the cookie jar, the console ring buffer and the send pipeline.
//! Frontends reach the workspace and the network only through this crate.
//!
//! Panic policy (AGENTS.md 4): [`Engine::send`] runs the pipeline inside `catch_unwind`; an
//! escaped panic becomes [`EngineError::Panicked`] and the session keeps its data. The UI calls
//! [`Engine::send_async`] from the runtime and never blocks its own thread.

pub mod console;
pub mod edit;
pub mod error;
pub mod pipeline;
pub mod runtime;

use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};

pub use error::{EngineError, Result};
pub use pipeline::{ResponseView, SendOptions, SendOutcome, SendTarget, SentRequest};

pub use postguy_http::Cancel;
use postguy_http::{Client, ClientConfig, CookieJar, HttpVersionPreference, ProxyRoute, StoredCookie, TlsMode};
use postguy_model::{HistoryEntry, Id, ProxySettings, RequestSettings, Settings};
use postguy_store::{StorePaths, Workspace};
use postguy_vars::Resolver;

use crate::console::ConsoleLog;
use crate::runtime::RuntimeHandle;

/// The session façade: cheap to clone, shared by the UI, the tools and the CLI.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    workspace: Workspace,
    runtime: RuntimeHandle,
    console: ConsoleLog,
    jar: Mutex<CookieJar>,
    settings: Mutex<Settings>,
    /// The newest finished sends, so `response_*` tools and the viewer can ask by history id.
    responses: Mutex<VecDeque<CachedResponse>>,
}

/// One send kept in memory for the response viewer and tools (bounded; 20 is plenty).
#[derive(Debug, Clone)]
pub struct CachedResponse {
    pub history_id: String,
    pub request: SentRequest,
    pub response: ResponseView,
    pub warnings: Vec<String>,
}

/// How many finished sends stay addressable by history id.
pub const RESPONSE_CACHE_CAP: usize = 20;

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").field("workspace", &self.inner.workspace.paths.workspace().display().to_string()).finish_non_exhaustive()
    }
}

impl Engine {
    /// Open a workspace, its settings and its persisted cookie jar. A corrupt settings file or
    /// jar already fell back to defaults inside the store/jar readers.
    pub fn open(paths: StorePaths) -> Result<Engine> {
        let workspace = Workspace::open(paths)?;
        let (cookie_value, _quarantine) = workspace.load_cookies()?;
        let settings = postguy_store::config_dir::load_settings(&workspace.paths)?.settings;
        let runtime =
            RuntimeHandle::new().map_err(|source| EngineError::BadRequest { reason: format!("cannot start the network runtime: {source}") })?;
        Ok(Engine {
            inner: Arc::new(Inner {
                workspace,
                runtime,
                console: ConsoleLog::new(),
                jar: Mutex::new(CookieJar::from_json(&cookie_value)),
                settings: Mutex::new(settings),
                responses: Mutex::new(VecDeque::new()),
            }),
        })
    }

    pub fn workspace(&self) -> &Workspace {
        &self.inner.workspace
    }

    pub fn store_paths(&self) -> &StorePaths {
        &self.inner.workspace.paths
    }

    pub fn console(&self) -> &ConsoleLog {
        &self.inner.console
    }

    /// The effective settings (a clone: callers never hold the lock across I/O).
    pub fn settings(&self) -> Settings {
        self.inner.settings.lock().map(|settings| settings.clone()).unwrap_or_default()
    }

    /// Apply and persist settings atomically.
    pub fn update_settings(&self, settings: Settings) -> Result<()> {
        postguy_store::config_dir::save_settings(&self.inner.workspace.paths, &settings)?;
        if let Ok(mut current) = self.inner.settings.lock() {
            *current = settings;
        }
        Ok(())
    }

    /// The cookie jar handle (guarded; the client locks it per hop).
    pub fn jar(&self) -> &Mutex<CookieJar> {
        &self.inner.jar
    }

    /// Persist the jar to `.postguy/cookies.json` atomically.
    pub fn save_jar(&self) -> Result<()> {
        let value = self.jar().lock().map(|jar| jar.to_json()).unwrap_or(serde_json::Value::Null);
        self.inner.workspace.save_cookies(&value)?;
        Ok(())
    }

    /// The cookie list for the manager and the `cookies_*` tools.
    pub fn cookies(&self) -> Vec<StoredCookie> {
        self.jar().lock().map(|jar| jar.cookies.clone()).unwrap_or_default()
    }

    /// Delete one cookie and persist.
    pub fn remove_cookie(&self, domain: &str, path: &str, name: &str) -> Result<()> {
        if let Ok(mut jar) = self.jar().lock() {
            jar.remove(domain, path, name);
        }
        self.save_jar()
    }

    /// Delete every cookie of a domain and persist.
    pub fn remove_cookie_domain(&self, domain: &str) -> Result<usize> {
        let removed = self.jar().lock().map(|mut jar| jar.remove_domain(domain)).unwrap_or(0);
        self.save_jar()?;
        Ok(removed)
    }

    pub fn clear_cookies(&self) -> Result<()> {
        if let Ok(mut jar) = self.jar().lock() {
            jar.clear();
        }
        self.save_jar()
    }

    /// The allowlist the cookie manager and scripts share.
    pub fn cookie_domains(&self) -> Vec<String> {
        self.jar().lock().map(|jar| jar.domain_allowlist.clone()).unwrap_or_default()
    }

    pub fn allow_cookie_domain(&self, domain: &str) -> Result<()> {
        if let Ok(mut jar) = self.jar().lock() {
            jar.allow_domain(domain);
        }
        self.save_jar()
    }

    pub fn disallow_cookie_domain(&self, domain: &str) -> Result<()> {
        if let Ok(mut jar) = self.jar().lock() {
            jar.disallow_domain(domain);
        }
        self.save_jar()
    }

    /// Build the HTTP client for one request's settings (TLS toggle, version, proxy, timeout).
    pub fn client_for(&self, request: &RequestSettings, global: &Settings) -> Result<Client> {
        let version = match request_version(request, global) {
            postguy_model::HttpVersion::Auto => HttpVersionPreference::Auto,
            postguy_model::HttpVersion::Http1 => HttpVersionPreference::Http1,
            postguy_model::HttpVersion::Http2 => HttpVersionPreference::Http2,
        };
        let tls = if request.verify_ssl { TlsMode::Verify { ca_pem_path: None } } else { TlsMode::Insecure };
        Client::new(ClientConfig { tls, version, proxy: global.proxy.clone(), ..ClientConfig::default() }).map_err(EngineError::Http)
    }

    /// The proxy route a URL would use under the current settings (Console and tests).
    pub fn proxy_route_for(&self, url: &str) -> Option<ProxyRoute> {
        let settings: ProxySettings = self.settings().proxy;
        postguy_http::ProxyPolicy::new(settings).route_for(url)
    }

    /// Send a request, blocking the caller until it completes. The pipeline is guarded so an
    /// internal panic becomes an error, never a crash (AGENTS.md 4).
    pub fn send(&self, target: SendTarget, options: SendOptions) -> Result<SendOutcome> {
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| self.inner.runtime.block_on(pipeline::execute(self, target, options))));
        match outcome {
            Ok(result) => result.inspect(|outcome| self.remember(outcome)),
            Err(payload) => {
                let message = panic_message(&payload);
                self.inner.console.push(crate::console::ConsoleLevel::Error, crate::console::ConsoleKind::Error, format!("send panicked: {message}"));
                Err(EngineError::Panicked { message })
            }
        }
    }

    /// Send from async code on the engine's runtime (the UI path). The caller polls the
    /// returned handle from its own event loop; the send itself never touches the UI thread.
    pub fn send_async(&self, target: SendTarget, options: SendOptions) -> tokio::task::JoinHandle<Result<SendOutcome>> {
        let engine = self.clone();
        self.inner.runtime.spawn(async move {
            let outcome = pipeline::execute(&engine, target, options).await?;
            engine.remember(&outcome);
            Ok(outcome)
        })
    }

    fn remember(&self, outcome: &SendOutcome) {
        if let Ok(mut responses) = self.inner.responses.lock() {
            responses.push_back(CachedResponse {
                history_id: outcome.history.id.0.clone(),
                request: outcome.request.clone(),
                response: outcome.response.clone(),
                warnings: outcome.warnings.clone(),
            });
            while responses.len() > RESPONSE_CACHE_CAP {
                responses.pop_front();
            }
        }
    }

    /// One finished send by history id, or the newest when `id` is None.
    pub fn response(&self, id: Option<&str>) -> Option<CachedResponse> {
        let responses = self.inner.responses.lock().ok()?;
        match id {
            Some(id) => responses.iter().find(|cached| cached.history_id == id).cloned(),
            None => responses.back().cloned(),
        }
    }

    /// Store one raw `Set-Cookie` header against a URL (the `cookies_set` tool) and persist.
    pub fn set_cookie(&self, url: &str, set_cookie: &str) -> Result<()> {
        if let Ok(mut jar) = self.jar().lock() {
            jar.store_from_headers(url, [set_cookie]);
        }
        self.save_jar()
    }

    /// Remove one history entry (the sidebar's Delete).
    pub fn delete_history(&self, id: &str) -> Result<bool> {
        Ok(self.inner.workspace.delete_history(id)?)
    }

    /// The workspace trash entries (deleted collections, environments and files).
    pub fn trash_entries(&self) -> Result<Vec<postguy_store::trash::Entry>> {
        Ok(self.inner.workspace.trash_entries()?)
    }

    /// Restore one trash entry by id; returns the restored path when it existed.
    pub fn restore_trash(&self, id: &str) -> Result<Option<std::path::PathBuf>> {
        Ok(self.inner.workspace.restore_from_trash(id)?)
    }

    /// Empty the history file atomically.
    pub fn clear_history(&self) -> Result<()> {
        self.inner.workspace.clear_history()?;
        Ok(())
    }

    /// The newest history entries (newest last), capped by the store's cap.
    pub fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        self.inner.workspace.read_history(limit).map_err(EngineError::Store)
    }

    pub fn console_entries(&self, limit: usize) -> Vec<crate::console::ConsoleEntry> {
        self.inner.console.entries(limit)
    }

    pub fn clear_console(&self) {
        self.inner.console.clear();
    }

    /// Resolve `{{variables}}` in a template against the current active environment and globals
    /// (the `variables_resolve` tool and the UI's preview share this).
    pub fn resolve_variables(&self, text: &str) -> Result<postguy_vars::Resolution> {
        self.resolve_variables_with(text, None, None)
    }

    /// Resolve with optional collection and environment context (the `variables_resolve` tool
    /// shows where each name came from). Secrets resolve but stay masked in the returned text?
    /// No: the tool needs the real text to explain a send, and its output is local; the caller
    /// gets `hits` with the `secret` flag so it can mask where it echoes.
    pub fn resolve_variables_with(&self, text: &str, collection: Option<&Id>, environment: Option<&Id>) -> Result<postguy_vars::Resolution> {
        let mut resolver = Resolver::new();
        let globals = self.inner.workspace.load_globals()?.into_value();
        let secrets = self.inner.workspace.secrets.values_for("globals").unwrap_or_default();
        resolver = resolver.with_scope(scope_from(&globals.variables, &secrets));
        if let Some(id) = collection {
            let collection = self.inner.workspace.load_collection(id)?.into_value();
            let secrets = self.inner.workspace.secrets.values_for(&id.0).unwrap_or_default();
            resolver = resolver.with_scope(collection_scope(&collection.variables, &secrets));
        }
        let environment_id = environment.cloned().or(self.inner.workspace.active_environment_id()?);
        if let Some(id) = environment_id {
            let environment = self.inner.workspace.load_environment(&id)?.into_value();
            let secrets = self.inner.workspace.secrets.values_for(&id.0).unwrap_or_default();
            resolver = resolver.with_scope(environment_scope(&environment.variables, &secrets));
        }
        Ok(resolver.resolve_text(text))
    }
}

fn request_version(request: &RequestSettings, global: &Settings) -> postguy_model::HttpVersion {
    // A stored request holds full values; `Auto` follows the global preference.
    match request.http_version {
        postguy_model::HttpVersion::Auto => global.http_version,
        other => other,
    }
}

fn scope_from(variables: &[postguy_model::Variable], secrets: &std::collections::BTreeMap<String, String>) -> postguy_vars::Scope {
    let mut scope = postguy_vars::Scope::new(
        postguy_vars::ScopeKind::Globals,
        variables.iter().filter(|variable| variable.enabled && !variable.key.is_empty()).map(|variable| {
            let value = if variable.secret { secrets.get(&variable.key).cloned().unwrap_or_default() } else { variable.value.clone() };
            (variable.key.clone(), value)
        }),
    );
    scope.secrets = variables.iter().filter(|variable| variable.secret).map(|variable| variable.key.clone()).collect();
    scope
}

fn environment_scope(variables: &[postguy_model::Variable], secrets: &std::collections::BTreeMap<String, String>) -> postguy_vars::Scope {
    let mut scope = postguy_vars::Scope::new(
        postguy_vars::ScopeKind::Environment,
        variables.iter().filter(|variable| variable.enabled && !variable.key.is_empty()).map(|variable| {
            let value = if variable.secret { secrets.get(&variable.key).cloned().unwrap_or_default() } else { variable.value.clone() };
            (variable.key.clone(), value)
        }),
    );
    scope.secrets = variables.iter().filter(|variable| variable.secret).map(|variable| variable.key.clone()).collect();
    scope
}

fn collection_scope(variables: &[postguy_model::Variable], secrets: &std::collections::BTreeMap<String, String>) -> postguy_vars::Scope {
    let mut scope = postguy_vars::Scope::new(
        postguy_vars::ScopeKind::Collection,
        variables.iter().filter(|variable| variable.enabled && !variable.key.is_empty()).map(|variable| {
            let value = if variable.secret { secrets.get(&variable.key).cloned().unwrap_or_default() } else { variable.value.clone() };
            (variable.key.clone(), value)
        }),
    );
    scope.secrets = variables.iter().filter(|variable| variable.secret).map(|variable| variable.key.clone()).collect();
    scope
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "an internal panicked value".to_string()
    }
}

/// Re-export the cancellation handle so frontends do not depend on `postguy-http` directly.
pub use postguy_http::Cancel as SendCancel;

/// A caller can build a cancellation handle without naming the HTTP crate.
pub fn new_cancel() -> Cancel {
    Cancel::new()
}
