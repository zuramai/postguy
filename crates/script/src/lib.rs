//! Postguy script sandbox (L3): runs pre-request and post-response scripts in an embedded
//! QuickJS engine with the Postman-style `pm` API and a Chai-style `pm.expect`.
//!
//! Safety model (AGENTS.md section 4): scripts get no filesystem, process or network access;
//! the only way out is [`Host`], which the engine implements on top of its own request
//! pipeline. Every run has a wall-clock limit, a memory limit, a stack limit and a
//! cancellation check, and [`run`] is total: an engine error or panic becomes a
//! [`ScriptFailure`], never a crash. The `pm` API is Rust closures, not a JS prelude.
//!
//! There is no cargo feature: `Settings.disable_scripts` is the off switch, applied by the
//! engine before it calls [`run`].
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod convert;
mod expect;
mod pm;
mod sandbox;

use std::collections::BTreeMap;
use std::sync::Arc;

use postguy_model::{NextRequest, ScriptFailure, ScriptPhase, TestResult};

pub use sandbox::run;

/// Hard limits for one [`run`].
#[derive(Debug, Clone)]
pub struct Limits {
    /// Wall-clock budget for all scripts of the run.
    pub timeout_ms: u64,
    /// Heap limit of the JS engine.
    pub memory_bytes: usize,
    /// Native stack the JS engine may use.
    pub stack_bytes: usize,
    /// How many `pm.sendRequest` calls a run may make.
    pub max_send_requests: u32,
    /// How many console lines are kept.
    pub max_log_lines: usize,
    /// How many `pm.test` results are kept.
    pub max_tests: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { timeout_ms: 5_000, memory_bytes: 64 << 20, stack_bytes: 1 << 20, max_send_requests: 10, max_log_lines: 1_000, max_tests: 1_000 }
    }
}

/// One script to run, with the label failures report (`"request Login"`).
#[derive(Debug, Clone)]
pub struct ScriptSource {
    pub origin: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub key: String,
    pub value: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ScriptBody {
    #[default]
    None,
    Raw(String),
    UrlEncoded(Vec<(String, String)>),
    FormData(Vec<(String, String)>),
    /// Binary, GraphQL or any other mode the script cannot edit.
    Other,
}

/// The request as scripts see (and, before sending, edit) it. Text is unresolved: `{{vars}}`
/// are still in place.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScriptRequest {
    pub id: String,
    pub name: String,
    pub method: String,
    pub url: String,
    pub headers: Vec<Header>,
    pub body: ScriptBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScriptResponse {
    pub code: u16,
    pub status: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub response_time_ms: u64,
    pub size: u64,
}

/// `pm.info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// `"prerequest"` or `"test"`.
    pub event_name: &'static str,
    pub iteration: u32,
    pub iteration_count: u32,
    pub request_name: String,
    pub request_id: String,
}

/// Every variable scope a script can see. The caller diffs the returned copy against what it
/// passed in to decide what to persist.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Variables {
    pub globals: BTreeMap<String, String>,
    pub collection: BTreeMap<String, String>,
    /// Read-only, merged outer to inner by the caller.
    pub folders: BTreeMap<String, String>,
    /// `(name, values)` of the active environment, or `None` when none is selected.
    pub environment: Option<(String, BTreeMap<String, String>)>,
    /// The current iteration's data row, read-only.
    pub data: Vec<(String, serde_json::Value)>,
    /// `pm.variables.set` values; never persisted.
    pub locals: BTreeMap<String, String>,
}

/// What the engine provides: the only way a script reaches the network or the cookie jar.
pub trait Host: Send + Sync {
    /// Send a request through the engine (`pm.sendRequest`). Blocking.
    fn send_request(&self, request: ScriptRequest) -> Result<ScriptResponse, String>;
    /// Cookies the jar would send to `url`, as name/value pairs.
    fn cookies_for(&self, url: &str) -> Vec<(String, String)>;
}

/// Polled while a script runs; `true` stops it.
pub type CancelCheck = Arc<dyn Fn() -> bool + Send + Sync>;

/// Everything one run needs.
pub struct Context {
    pub phase: ScriptPhase,
    pub info: Info,
    pub variables: Variables,
    pub request: ScriptRequest,
    pub response: Option<ScriptResponse>,
    pub host: Arc<dyn Host>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub level: LogLevel,
    pub message: String,
}

/// What a run produced.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// Final state of every scope.
    pub variables: Variables,
    /// Final state; only a pre-request script can change it.
    pub request: ScriptRequest,
    pub tests: Vec<TestResult>,
    pub failures: Vec<ScriptFailure>,
    pub logs: Vec<LogLine>,
    pub next_request: NextRequest,
    pub skip_request: bool,
}
