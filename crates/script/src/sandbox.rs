//! One sandboxed run: a fresh QuickJS runtime with memory, stack and wall-clock limits, the
//! scripts evaluated in order, then the queued promise jobs and timers drained.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use postguy_model::{NextRequest, ScriptFailure, ScriptPhase};
use rquickjs::context::EvalOptions;
use rquickjs::{Array, CatchResultExt, Ctx, Function, Object, Runtime, Value};

use crate::pm::{self, MAX_TIMERS, Shared, State, TIMERS};
use crate::{CancelCheck, Context, Limits, Outcome, ScriptSource, Variables};

const REASON_NONE: u8 = 0;
const REASON_TIMEOUT: u8 = 1;
const REASON_CANCELLED: u8 = 2;
/// Most promise jobs executed per run, so a self-feeding microtask loop ends.
const MAX_JOBS: usize = 1_000_000;
/// Timer rounds (timers scheduled from timers) per run.
const MAX_TIMER_ROUNDS: usize = 100;

/// Run `sources` in order in one QuickJS context. Total: errors and panics become failures.
pub fn run(sources: &[ScriptSource], context: Context, limits: &Limits, cancelled: CancelCheck) -> Outcome {
    let fallback_variables = context.variables.clone();
    let fallback_request = context.request.clone();
    let phase = context.phase;
    let origin = sources.first().map(|s| s.origin.clone()).unwrap_or_default();
    match catch_unwind(AssertUnwindSafe(|| run_inner(sources, context, limits, cancelled))) {
        Ok(outcome) => outcome,
        Err(panic) => {
            let text = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            Outcome {
                variables: fallback_variables,
                request: fallback_request,
                tests: Vec::new(),
                failures: vec![ScriptFailure { phase, origin, message: format!("the script engine failed: {text}"), line: None }],
                logs: Vec::new(),
                next_request: NextRequest::Unchanged,
                skip_request: false,
            }
        }
    }
}

fn engine_failure(context: Context, origin: &str, message: String) -> Outcome {
    Outcome {
        variables: context.variables,
        request: context.request,
        tests: Vec::new(),
        failures: vec![ScriptFailure { phase: context.phase, origin: origin.to_string(), message, line: None }],
        logs: Vec::new(),
        next_request: NextRequest::Unchanged,
        skip_request: false,
    }
}

fn run_inner(sources: &[ScriptSource], context: Context, limits: &Limits, cancelled: CancelCheck) -> Outcome {
    let origin = sources.first().map(|s| s.origin.clone()).unwrap_or_default();
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(err) => return engine_failure(context, &origin, format!("the script engine could not start: {err}")),
    };
    runtime.set_memory_limit(limits.memory_bytes);
    runtime.set_max_stack_size(limits.stack_bytes);
    let reason = Arc::new(AtomicU8::new(REASON_NONE));
    let deadline = Instant::now() + Duration::from_millis(limits.timeout_ms);
    {
        let reason = reason.clone();
        let cancelled = cancelled.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            if cancelled() {
                reason.store(REASON_CANCELLED, Ordering::Relaxed);
                return true;
            }
            if Instant::now() >= deadline {
                reason.store(REASON_TIMEOUT, Ordering::Relaxed);
                return true;
            }
            false
        })));
    }
    let js_context = match rquickjs::Context::full(&runtime) {
        Ok(c) => c,
        Err(err) => return engine_failure(context, &origin, format!("the script engine could not start: {err}")),
    };

    let state: Shared = Rc::new(RefCell::new(State {
        phase: context.phase,
        info: context.info,
        vars: context.variables,
        request: context.request,
        response: context.response,
        host: context.host,
        limits: limits.clone(),
        cancelled: cancelled.clone(),
        tests: Vec::new(),
        failures: Vec::new(),
        logs: Vec::new(),
        sends: 0,
        next: NextRequest::Unchanged,
        skip: false,
        tests_overflowed: false,
        origin: origin.clone(),
    }));

    let phase = context.phase;
    let timeout_ms = limits.timeout_ms;
    let memory_mib = limits.memory_bytes >> 20;
    let failure_of = |origin: &str, message: String, line: Option<u32>| ScriptFailure { phase, origin: origin.to_string(), message, line };

    js_context.with(|ctx| {
        if let Err(err) = pm::install(&ctx, &state) {
            let message = describe(&ctx, err, &reason, timeout_ms, memory_mib).0;
            push_failure(&state, failure_of(&origin, format!("the script engine could not start: {message}"), None));
            return;
        }
        for source in sources {
            // Each script runs in its own function so `const`/`let` names never collide
            // between the collection, folder and request scripts. No newline is added, so
            // line numbers still match the editor.
            let wrapped = format!("(function(){{{}\n}})();", source.text);
            let mut options = EvalOptions::default();
            options.global = true;
            options.strict = false;
            options.filename = Some(source.origin.clone());
            if let Err(err) = ctx.eval_with_options::<Value, _>(wrapped, options).catch(&ctx).map(|_| ()).map_err(|e| e.to_string()) {
                let (message, line) = split_caught(&err, &source.origin);
                let message = limit_message(&message, &reason, timeout_ms, memory_mib);
                push_failure(&state, failure_of(&source.origin, message, line));
                if phase == ScriptPhase::PreRequest {
                    break;
                }
                if reason.load(Ordering::Relaxed) != REASON_NONE {
                    break;
                }
                continue;
            }
            if let Err(message) = drain(&ctx, &reason, deadline) {
                let message = limit_message(&message, &reason, timeout_ms, memory_mib);
                push_failure(&state, failure_of(&source.origin, message, None));
                if phase == ScriptPhase::PreRequest || reason.load(Ordering::Relaxed) != REASON_NONE {
                    break;
                }
            }
        }
    });

    drop(js_context);
    drop(runtime);
    let state = match Rc::try_unwrap(state) {
        Ok(cell) => cell.into_inner(),
        Err(shared) => match shared.try_borrow() {
            Ok(state) => State {
                phase: state.phase,
                info: state.info.clone(),
                vars: state.vars.clone(),
                request: state.request.clone(),
                response: state.response.clone(),
                host: state.host.clone(),
                limits: state.limits.clone(),
                cancelled: state.cancelled.clone(),
                tests: state.tests.clone(),
                failures: state.failures.clone(),
                logs: state.logs.clone(),
                sends: state.sends,
                next: state.next.clone(),
                skip: state.skip,
                tests_overflowed: state.tests_overflowed,
                origin: state.origin.clone(),
            },
            Err(_) => {
                return Outcome {
                    variables: Variables::default(),
                    request: Default::default(),
                    tests: Vec::new(),
                    failures: vec![ScriptFailure {
                        phase,
                        origin,
                        message: "the script engine failed: its state was still in use".to_string(),
                        line: None,
                    }],
                    logs: Vec::new(),
                    next_request: NextRequest::Unchanged,
                    skip_request: false,
                };
            }
        },
    };
    let mut tests = state.tests;
    // A promise-returning test that never settled keeps its placeholder message.
    for test in &mut tests {
        if test.message.as_deref() == Some("the test did not finish") {
            test.message = Some("the test did not finish".to_string());
        }
    }
    Outcome {
        variables: state.vars,
        request: state.request,
        tests,
        failures: state.failures,
        logs: state.logs,
        next_request: state.next,
        skip_request: state.skip,
    }
}

fn push_failure(state: &Shared, failure: ScriptFailure) {
    if let Ok(mut state) = state.try_borrow_mut() {
        state.failures.push(failure);
    }
}

/// Map an engine message to the limit that caused it, when one did.
fn limit_message(message: &str, reason: &AtomicU8, timeout_ms: u64, memory_mib: usize) -> String {
    match reason.load(Ordering::Relaxed) {
        REASON_TIMEOUT => return format!("the script ran longer than {timeout_ms} ms"),
        REASON_CANCELLED => return "the script was cancelled".to_string(),
        _ => {}
    }
    if message.contains("out of memory") {
        return format!("the script used more than {memory_mib} MiB of memory");
    }
    message.to_string()
}

/// `(message, line)` from a caught error, mapped through the limit reasons.
fn describe<'js>(ctx: &Ctx<'js>, err: rquickjs::Error, reason: &AtomicU8, timeout_ms: u64, memory_mib: usize) -> (String, Option<u32>) {
    let text = match err {
        rquickjs::Error::Exception => {
            let caught = ctx.catch();
            pm::error_message(&caught)
        }
        other => other.to_string(),
    };
    (limit_message(&text, reason, timeout_ms, memory_mib), None)
}

/// `CatchResultExt` renders a caught exception as `Error: message\n  at file:line:col`; split
/// it back into the message and the first line number that belongs to `origin`.
fn split_caught(rendered: &str, origin: &str) -> (String, Option<u32>) {
    let mut message = String::new();
    let mut line = None;
    for text_line in rendered.lines() {
        let trimmed = text_line.trim();
        if trimmed.starts_with("at ") {
            if line.is_none() {
                line = line_of(trimmed, origin);
            }
            continue;
        }
        if !message.is_empty() {
            message.push(' ');
        }
        message.push_str(trimmed);
    }
    let message = message.strip_prefix("Error: ").unwrap_or(&message).to_string();
    (message, line)
}

fn line_of(frame: &str, origin: &str) -> Option<u32> {
    let at = frame.find(origin)?;
    let rest = frame.get(at + origin.len()..)?;
    let rest = rest.strip_prefix(':')?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Run queued promise jobs and timers until both are empty, or a limit trips.
fn drain<'js>(ctx: &Ctx<'js>, reason: &AtomicU8, deadline: Instant) -> Result<(), String> {
    let rounds = MAX_TIMER_ROUNDS;
    for _ in 0..rounds {
        let mut jobs = 0usize;
        while ctx.execute_pending_job() {
            jobs += 1;
            if jobs > MAX_JOBS {
                return Err("too many queued promise jobs".to_string());
            }
            if reason.load(Ordering::Relaxed) != REASON_NONE || Instant::now() >= deadline {
                return Err("interrupted".to_string());
            }
        }
        let timers: Array = ctx.globals().get(TIMERS).map_err(|e| e.to_string())?;
        if timers.is_empty() {
            return Ok(());
        }
        let mut entries: Vec<(f64, f64, Object<'js>)> = Vec::new();
        for entry in timers.iter::<Object>().flatten().take(MAX_TIMERS) {
            if entry.get::<_, bool>("cancelled").unwrap_or(false) {
                continue;
            }
            let ms = entry.get::<_, f64>("ms").unwrap_or(0.0);
            let id = entry.get::<_, f64>("id").unwrap_or(0.0);
            entries.push((ms, id, entry));
        }
        ctx.globals().set(TIMERS, Array::new(ctx.clone()).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        entries.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        for (_, _, entry) in entries {
            let callback: Function = entry.get("f").map_err(|e| e.to_string())?;
            let args: Array = entry.get("args").map_err(|e| e.to_string())?;
            let list: Vec<Value> = args.iter::<Value>().flatten().collect();
            let call = match list.len() {
                0 => callback.call::<_, Value>(()),
                _ => callback.call::<_, Value>((rquickjs::function::Rest(list),)),
            };
            if let Err(err) = call.catch(ctx) {
                return Err(split_caught(&err.to_string(), "").0);
            }
        }
    }
    Err("setTimeout callbacks kept scheduling more callbacks".to_string())
}
