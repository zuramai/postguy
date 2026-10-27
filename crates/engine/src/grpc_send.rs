//! The full gRPC send: pre-invoke script, variable resolution, inherited auth as metadata, the call
//! (unary or streaming, chosen from the method), on-message and post-response scripts and the Test
//! tab. The sibling of the HTTP pipeline; script slots come from [`crate::scripts::ScriptSlot`].

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::time::Instant;

use postguy_grpc::GrpcError;
use postguy_http::Cancel;
use postguy_model::{GrpcRequest, Id, KeyValue, NextRequest, ScriptFailure, ScriptPhase};
use postguy_script as js;
use postguy_util::json::{Caps, parse_json_str};
use serde::Serialize;

use crate::Engine;
use crate::console::{ConsoleKind, ConsoleLevel};
use crate::error::{EngineError, Result};
use crate::grpc::{apply_grpc_auth, resolve_grpc_payload, resolve_grpc_target};
use crate::pipeline::{Located, SendOptions, load_scopes};
use crate::scripts::{self, PhaseInput, PhaseOutput, ScriptReport, ScriptSlot};

/// Most messages the on-message script runs for; later ones are skipped with a warning.
pub const MAX_ON_MESSAGE_RUNS: usize = 1000;

/// Knobs for one gRPC send.
#[derive(Debug, Clone, Default)]
pub struct GrpcSendOptions {
    /// Explicit environment; None uses the workspace's active environment.
    pub environment: Option<Id>,
    /// The caller's cancellation handle.
    pub cancel: Option<Cancel>,
    /// Run the pre-invoke, on-message and post-response scripts and the Test tab (unless Settings disables scripts).
    pub run_scripts: bool,
}

/// The terminal gRPC status of a call.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrpcStatus {
    pub code: i32,
    /// Canonical name, for example `OK` or `NOT_FOUND`.
    pub name: String,
    pub message: String,
}

/// One completed gRPC send. A non-OK status is a result, not an error.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrpcSendOutcome {
    pub status: GrpcStatus,
    pub metadata: Vec<(String, String)>,
    /// Every response message in arrival order (empty on a non-OK status).
    pub messages: Vec<serde_json::Value>,
    pub client_streaming: bool,
    pub server_streaming: bool,
    pub elapsed_ms: u64,
    pub script: ScriptReport,
    pub warnings: Vec<String>,
}

impl Engine {
    /// Invoke a gRPC request through the full pipeline, blocking until it completes. The draft is
    /// sent as given (unsaved edits included). An internal panic becomes an error, never a crash.
    pub fn grpc_send(&self, collection: &Id, request: &Id, draft: &GrpcRequest, options: GrpcSendOptions) -> Result<GrpcSendOutcome> {
        let outcome =
            std::panic::catch_unwind(AssertUnwindSafe(|| self.inner.runtime.block_on(execute_grpc(self, collection, request, draft, options))));
        match outcome {
            Ok(result) => result,
            Err(payload) => {
                let message = crate::panic_message(&payload);
                self.console().push(ConsoleLevel::Error, ConsoleKind::Error, format!("gRPC send panicked: {message}"));
                Err(EngineError::Panicked { message })
            }
        }
    }
}

fn cancelled() -> EngineError {
    EngineError::Http(postguy_http::HttpError::Cancelled)
}

fn enabled_metadata(request: &GrpcRequest) -> Vec<(String, String)> {
    request.metadata.iter().filter(|row| row.enabled && !row.key.is_empty()).map(|row| (row.key.clone(), row.value.clone())).collect()
}

/// The request a pre-invoke script sees: the URL, the metadata as headers and the message as the raw body.
fn grpc_script_request(located: &Located, request: &GrpcRequest) -> js::ScriptRequest {
    js::ScriptRequest {
        id: located.request_id.clone(),
        name: located.request_name.clone(),
        method: "POST".into(),
        url: request.url.clone(),
        headers: request
            .metadata
            .iter()
            .filter(|row| !row.key.is_empty())
            .map(|row| js::Header { key: row.key.clone(), value: row.value.clone(), enabled: row.enabled })
            .collect(),
        body: js::ScriptBody::Raw(request.message.clone()),
    }
}

/// Copy the edits a pre-invoke script made onto the request about to be sent. The method is ignored.
fn apply_grpc_request(request: &mut GrpcRequest, before: &js::ScriptRequest, after: &js::ScriptRequest) {
    if after.url != before.url {
        request.url = after.url.clone();
    }
    if after.headers != before.headers {
        request.metadata = after
            .headers
            .iter()
            .map(|header| KeyValue { key: header.key.clone(), value: header.value.clone(), enabled: header.enabled, ..Default::default() })
            .collect();
    }
    if after.body != before.body
        && let js::ScriptBody::Raw(text) = &after.body
    {
        request.message = text.clone();
    }
}

/// Fold one non-pre phase's result into the report. Returns the phase output when it ran cleanly enough to use.
fn absorb_phase(
    result: Result<Option<PhaseOutput>>,
    label: &str,
    request_name: &str,
    report: &mut ScriptReport,
    warnings: &mut Vec<String>,
) -> Option<PhaseOutput> {
    match result {
        Ok(Some(mut phase)) => {
            report.tests.append(&mut phase.outcome.tests);
            let report_fields = std::mem::take(&mut phase.outcome.report);
            report.report.extend(report_fields);
            for failure in std::mem::take(&mut phase.outcome.failures) {
                warnings.push(format!("{label} script in {} failed: {}", failure.origin, failure.message));
                report.failures.push(failure);
            }
            if phase.outcome.next_request != NextRequest::Unchanged {
                report.next_request = phase.outcome.next_request.clone();
            }
            report.variables_changed |= phase.persisted;
            report.collection_changed |= phase.collection_changed;
            Some(phase)
        }
        Ok(None) => None,
        Err(err) => {
            warnings.push(format!("{label} scripts did not run: {err}"));
            report.failures.push(ScriptFailure {
                phase: ScriptPhase::PostResponse,
                origin: format!("request {request_name}"),
                message: err.to_string(),
                line: None,
            });
            None
        }
    }
}

async fn execute_grpc(engine: &Engine, collection: &Id, request: &Id, draft: &GrpcRequest, options: GrpcSendOptions) -> Result<GrpcSendOutcome> {
    let global = engine.settings();
    let mut working = draft.clone();
    let mut located = engine.grpc_located(collection, request, draft)?;
    let mut scopes = load_scopes(engine, &located, options.environment.as_ref(), None)?;
    let mut locals: BTreeMap<String, String> = BTreeMap::new();
    let mut report = ScriptReport::default();
    let mut warnings: Vec<String> = Vec::new();
    let cancel = options.cancel.clone().unwrap_or_default();
    let phase_options = SendOptions {
        environment: options.environment.clone(),
        cancel: Some(cancel.clone()),
        run_scripts: options.run_scripts,
        ..Default::default()
    };

    let scripts_on = options.run_scripts && !global.disable_scripts;
    if options.run_scripts && global.disable_scripts {
        let scripts = &working.scripts;
        let any = [&scripts.before_invoke, &scripts.on_message, &scripts.after_response]
            .into_iter()
            .any(|script| script.as_ref().is_some_and(|script| !script.is_empty()));
        if any || working.tests.has_work() {
            warnings.push("scripts are disabled in Settings; gRPC scripts and tests were skipped".to_string());
        }
    }

    // 2. Pre-invoke.
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    if scripts_on {
        let before = grpc_script_request(&located, &working);
        let input = PhaseInput {
            slot: ScriptSlot::BeforeInvoke,
            scopes: &scopes,
            options: &phase_options,
            locals: &locals,
            request: before.clone(),
            response: None,
        };
        if let Some(phase) = scripts::run_phase(engine, &mut located, input, &mut warnings).await? {
            let outcome = phase.outcome;
            if let Some(failure) = outcome.failures.first() {
                let message = postguy_vars::mask_secrets(&failure.message, &scopes.scopes);
                let error =
                    EngineError::Script { phase: failure.phase.label().to_string(), origin: failure.origin.clone(), message, line: failure.line };
                engine.console().push(ConsoleLevel::Error, ConsoleKind::Error, error.to_string());
                return Err(error);
            }
            if outcome.skip_request {
                return Err(EngineError::Skipped { reason: "the pre-invoke script called pm.execution.skipRequest()".to_string() });
            }
            apply_grpc_request(&mut working, &before, &outcome.request);
            locals = outcome.variables.locals.clone();
            report.tests.extend(outcome.tests);
            report.report.extend(outcome.report);
            if outcome.next_request != NextRequest::Unchanged {
                report.next_request = outcome.next_request;
            }
            report.variables_changed |= phase.persisted;
            report.collection_changed |= phase.collection_changed;
            if phase.scopes_changed {
                scopes = load_scopes(engine, &located, options.environment.as_ref(), None)?;
            }
        }
    }

    // 3. Variables and auth.
    let resolver = scripts::resolver_with_locals(&scopes, &locals);
    resolve_grpc_target(&resolver, &mut working)?;
    resolve_grpc_payload(&resolver, &mut working)?;
    apply_grpc_auth(engine, &located, &resolver, &mut working, &mut warnings).await?;

    // 4. Descriptors, method and input messages.
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    if working.method_path.trim().is_empty() {
        return Err(EngineError::BadRequest { reason: "choose a method first: load the service definition and pick a method".into() });
    }
    let pool = engine.grpc_pool_async(&working, false).await?;
    let method = postguy_grpc::method(&pool, &working.method_path).map_err(|err| EngineError::BadRequest { reason: err.to_string() })?;
    let (client_streaming, server_streaming) = (method.is_client_streaming(), method.is_server_streaming());
    let text = if working.message.trim().is_empty() { "{}" } else { working.message.as_str() };
    let value = parse_json_str(text, &Caps::default())
        .map_err(|err| EngineError::BadRequest { reason: format!("the request message is not valid JSON: {err}") })?;
    let inputs: Vec<serde_json::Value> = match value {
        serde_json::Value::Array(items) if client_streaming => items,
        serde_json::Value::Array(_) => {
            return Err(EngineError::BadRequest { reason: "this method takes one message; write a JSON object, not an array".into() });
        }
        other => vec![other],
    };
    let (endpoint, tls) = engine.grpc_connection(&working)?;
    let metadata = enabled_metadata(&working);

    // 5. The call.
    let started = Instant::now();
    let call = async {
        match (client_streaming, server_streaming) {
            (false, false) => {
                let first = inputs.into_iter().next().unwrap_or(serde_json::Value::Null);
                postguy_grpc::unary(&endpoint, &tls, &method, first, &metadata).await.map(|result| (vec![result.json], result.metadata))
            }
            (false, true) => {
                let first = inputs.into_iter().next().unwrap_or(serde_json::Value::Null);
                postguy_grpc::server_streaming(&endpoint, &tls, &method, first, &metadata).await.map(|result| (result.messages, result.metadata))
            }
            (true, false) => {
                postguy_grpc::client_streaming(&endpoint, &tls, &method, inputs, &metadata).await.map(|result| (vec![result.json], result.metadata))
            }
            (true, true) => {
                postguy_grpc::bidi_streaming(&endpoint, &tls, &method, inputs, &metadata).await.map(|result| (result.messages, result.metadata))
            }
        }
    };
    let result = tokio::select! {
        result = call => result,
        _ = cancel.cancelled() => return Err(cancelled()),
    };
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let (messages, response_metadata, status) = match result {
        Ok((messages, metadata)) => (messages, metadata, GrpcStatus { code: 0, name: "OK".into(), message: String::new() }),
        Err(GrpcError::Status { code, message }) => {
            (Vec::new(), Vec::new(), GrpcStatus { code, name: postguy_grpc::status_name(code).into(), message })
        }
        Err(other) => return Err(EngineError::BadRequest { reason: other.to_string() }),
    };

    // 6. On-message, once per received message.
    if scripts_on && working.scripts.on_message.as_ref().is_some_and(|script| !script.is_empty()) && !messages.is_empty() {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        for message in messages.iter().take(MAX_ON_MESSAGE_RUNS) {
            let body = serde_json::to_vec(message).unwrap_or_default();
            let response = js::ScriptResponse {
                code: u16::try_from(status.code).unwrap_or(2),
                status: status.name.clone(),
                headers: response_metadata.clone(),
                size: u64::try_from(body.len()).unwrap_or(u64::MAX),
                body,
                response_time_ms: elapsed_ms,
            };
            let input = PhaseInput {
                slot: ScriptSlot::OnMessage,
                scopes: &scopes,
                options: &phase_options,
                locals: &locals,
                request: grpc_script_request(&located, &working),
                response: Some(response),
            };
            let result = scripts::run_phase(engine, &mut located, input, &mut warnings).await;
            if let Some(phase) = absorb_phase(result, "on-message", &located.request_name.clone(), &mut report, &mut warnings) {
                locals = phase.outcome.variables.locals.clone();
                if phase.scopes_changed {
                    scopes = load_scopes(engine, &located, options.environment.as_ref(), None)?;
                }
            }
        }
        if messages.len() > MAX_ON_MESSAGE_RUNS {
            warnings.push(format!("the on-message script ran for the first {MAX_ON_MESSAGE_RUNS} of {} messages", messages.len()));
        }
    }

    // 7. Post-response script and the Test tab. A failure here never discards the response.
    if scripts_on {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        let body = if status.code != 0 {
            status.message.clone().into_bytes()
        } else if server_streaming {
            serde_json::to_vec(&messages).unwrap_or_default()
        } else {
            messages.first().and_then(|message| serde_json::to_vec(message).ok()).unwrap_or_default()
        };
        let response = js::ScriptResponse {
            code: u16::try_from(status.code).unwrap_or(2),
            status: status.name.clone(),
            headers: response_metadata.clone(),
            size: u64::try_from(body.len()).unwrap_or(u64::MAX),
            body,
            response_time_ms: elapsed_ms,
        };
        let input = PhaseInput {
            slot: ScriptSlot::AfterResponse,
            scopes: &scopes,
            options: &phase_options,
            locals: &locals,
            request: grpc_script_request(&located, &working),
            response: Some(response),
        };
        let result = scripts::run_phase(engine, &mut located, input, &mut warnings).await;
        absorb_phase(result, "post-response", &located.request_name.clone(), &mut report, &mut warnings);
        for test in &mut report.tests {
            test.message = test.message.take().map(|message| postguy_vars::mask_secrets(&message, resolver.scopes()));
        }
        for failure in &mut report.failures {
            failure.message = postguy_vars::mask_secrets(&failure.message, resolver.scopes());
        }
    }

    Ok(GrpcSendOutcome { status, metadata: response_metadata, messages, client_streaming, server_streaming, elapsed_ms, script: report, warnings })
}
