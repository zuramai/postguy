//! Collection runs: plan the requests, iterate the data rows, send each HTTP request through
//! the normal pipeline (scripts included), follow `setNextRequest`, stop on error or cancel,
//! and store the result under `.postguy/runs/`.

use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::time::{Duration, Instant};

use postguy_http::Cancel;
use postguy_model::{Collection, Id, Item, NextRequest, RequestKind};
use postguy_runner::{
    DATA_FILE_CAP, DataSet, IterationResult, PlannedRequest, RequestRun, RunConfig, RunError, RunEvent, RunResult, RunSummary, Sequencer, Totals,
};

use crate::Engine;
use crate::console::{ConsoleKind, ConsoleLevel};
use crate::error::{EngineError, Result};
use crate::pipeline::{IterationScope, SendOptions, SendTarget};

fn run_error(err: RunError) -> EngineError {
    EngineError::Run { message: err.to_string() }
}

/// Read a CSV or JSON data file (capped at 10 MiB). The file name picks the format.
pub fn load_data_file(path: &Path) -> Result<DataSet> {
    let bytes = postguy_util::read_file_capped(path, DATA_FILE_CAP)?;
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("data.csv");
    postguy_runner::parse_data(&bytes, name).map_err(run_error)
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// `(method, url)` of a stored HTTP request, for error rows where no send happened.
fn request_head(collection: &Collection, id: &Id) -> (String, String) {
    let mut stack: Vec<&Item> = collection.items.iter().collect();
    let mut visited = 0usize;
    while let Some(item) = stack.pop() {
        visited = visited.saturating_add(1);
        if visited > 200_000 {
            break;
        }
        match item {
            Item::Folder(folder) => stack.extend(folder.items.iter()),
            Item::Request(request) if request.id == *id => {
                return match &request.kind {
                    RequestKind::Http(http) => (postguy_http::build::method_text(http.method).to_string(), http.url.clone()),
                    _ => (String::new(), String::new()),
                };
            }
            Item::Request(_) => {}
        }
    }
    (String::new(), String::new())
}

/// Sleep in slices so a cancel is noticed; returns false when cancelled.
fn pause(delay_ms: u64, cancel: &Cancel) -> bool {
    let deadline = Instant::now() + Duration::from_millis(delay_ms);
    while Instant::now() < deadline {
        if cancel.is_cancelled() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())));
    }
    !cancel.is_cancelled()
}

impl Engine {
    /// Run a collection (or one folder, or a subset of requests) and store the result.
    /// Blocking: call it from a non-runtime thread, like [`Engine::send`]. `on_event` is called
    /// as each request finishes. An internal panic becomes an error, never a crash.
    pub fn run_collection(
        &self,
        config: &RunConfig,
        data: Option<DataSet>,
        cancel: &Cancel,
        on_event: &mut dyn FnMut(&RunEvent),
    ) -> Result<RunResult> {
        match std::panic::catch_unwind(AssertUnwindSafe(|| self.run_collection_inner(config, data, cancel, on_event))) {
            Ok(result) => result,
            Err(payload) => {
                let message = panic_text(payload.as_ref());
                self.console().push(ConsoleLevel::Error, ConsoleKind::Error, format!("run panicked: {message}"));
                Err(EngineError::Panicked { message })
            }
        }
    }

    fn run_collection_inner(
        &self,
        config: &RunConfig,
        data: Option<DataSet>,
        cancel: &Cancel,
        on_event: &mut dyn FnMut(&RunEvent),
    ) -> Result<RunResult> {
        if !self.workspace().collection_ids()?.contains(&config.collection) {
            return Err(EngineError::NotFound { what: "collection".into(), id: config.collection.0.clone() });
        }
        let collection = self.collection(&config.collection)?;
        let planned = postguy_runner::plan(&collection, config.folder.as_ref(), config.requests.as_deref()).map_err(run_error)?;
        if planned.is_empty() {
            return Err(EngineError::Run { message: "there are no requests to run; pick a collection or folder that contains requests".to_string() });
        }
        let rows = data.as_ref().map(|set| set.rows.as_slice()).unwrap_or_default();
        let iterations = config.effective_iterations(data.as_ref().map(|set| set.rows.len())).map_err(run_error)?;
        let environment_name = config
            .environment
            .clone()
            .or_else(|| self.workspace().active_environment_id().ok().flatten())
            .and_then(|id| self.environment(&id).ok())
            .map(|environment| environment.name);

        let started_ms = postguy_util::time::now_unix_ms();
        let short: String = Id::generate().0.chars().take(8).collect();
        let run_id = format!("{:013}-{short}", started_ms.max(0));
        let requests_per_iteration = planned.len();
        let mut sequencer = Sequencer::new(planned);
        let mut totals = Totals::default();
        let mut finished: Vec<IterationResult> = Vec::new();
        let mut stopped: Option<String> = None;
        on_event(&RunEvent::Started { iterations, requests_per_iteration });

        'iterations: for index in 0..iterations {
            if cancel.is_cancelled() {
                stopped = Some("cancelled".to_string());
                break;
            }
            let row = usize::try_from(index).ok().and_then(|at| rows.get(at)).cloned().unwrap_or_default();
            let scope = IterationScope { index, count: iterations, data: row };
            let mut iteration = IterationResult { index, ..IterationResult::default() };
            let mut current = sequencer.start();
            while let Some(position) = current {
                if cancel.is_cancelled() {
                    stopped = Some("cancelled".to_string());
                    iteration.warnings = sequencer.take_warnings();
                    finished.push(iteration);
                    break 'iterations;
                }
                let Some(step) = sequencer.order().get(position).cloned() else { break };
                let (run, directive) = self.run_one(&collection, &step, config, &scope, cancel);
                on_event(&RunEvent::Request { iteration: index, run: Box::new(run.clone()) });
                totals.add(&run);
                let failed = run.failed();
                let name = run.name.clone();
                iteration.requests.push(run);
                if cancel.is_cancelled() {
                    stopped = Some("cancelled".to_string());
                    iteration.warnings = sequencer.take_warnings();
                    finished.push(iteration);
                    break 'iterations;
                }
                if config.stop_on_error && failed {
                    stopped = Some(format!("stopped after {name} failed (stop on error)"));
                    iteration.warnings = sequencer.take_warnings();
                    finished.push(iteration);
                    break 'iterations;
                }
                current = sequencer.next(&directive);
                let more = current.is_some() || index.saturating_add(1) < iterations;
                if more && config.delay_ms > 0 && !pause(config.delay_ms, cancel) {
                    stopped = Some("cancelled".to_string());
                    iteration.warnings = sequencer.take_warnings();
                    finished.push(iteration);
                    break 'iterations;
                }
            }
            iteration.warnings = sequencer.take_warnings();
            finished.push(iteration);
            on_event(&RunEvent::IterationFinished { index });
        }

        let mut result = RunResult {
            id: run_id.clone(),
            collection_id: collection.id.clone(),
            collection_name: collection.name.clone(),
            folder: config.folder.clone(),
            environment_name,
            started_ms,
            finished_ms: postguy_util::time::now_unix_ms(),
            iterations: finished,
            totals,
            stopped,
        };
        let stored = serde_json::to_vec(&result)
            .map_err(|err| EngineError::Run { message: format!("could not encode the run: {err}") })
            .and_then(|bytes| self.workspace().save_run(&collection.id.0, &run_id, &bytes).map_err(EngineError::from));
        if let Err(err) = stored
            && let Some(last) = result.iterations.last_mut()
        {
            last.warnings.push(format!("could not save the run: {err}"));
        }
        let summary = format!(
            "run {} finished: {}/{} tests passed, {} failed requests",
            result.collection_name,
            result.totals.tests_passed,
            result.totals.tests_passed.saturating_add(result.totals.tests_failed).saturating_add(result.totals.tests_skipped),
            result.totals.failed_requests
        );
        self.console().push(if result.passed() { ConsoleLevel::Info } else { ConsoleLevel::Warning }, ConsoleKind::Log, summary);
        on_event(&RunEvent::Finished);
        Ok(result)
    }

    /// Send one planned request; its row and the script's next-request directive come back.
    fn run_one(
        &self,
        collection: &Collection,
        step: &PlannedRequest,
        config: &RunConfig,
        scope: &IterationScope,
        cancel: &Cancel,
    ) -> (RequestRun, NextRequest) {
        let base = |method: String, url: String| RequestRun {
            request_id: step.id.clone(),
            name: step.name.clone(),
            path: step.path.clone(),
            method,
            url,
            status: None,
            duration_ms: 0,
            size_bytes: 0,
            tests: Vec::new(),
            failures: Vec::new(),
            error: None,
            skipped: None,
        };
        if step.protocol != "http" {
            let (method, url) = request_head(collection, &step.id);
            let mut run = base(method, url);
            run.skipped = Some("only HTTP requests run in the collection runner".to_string());
            return (run, NextRequest::Unchanged);
        }
        let options =
            SendOptions { environment: config.environment.clone(), cancel: Some(cancel.clone()), run_scripts: true, iteration: Some(scope.clone()) };
        match self.send(SendTarget::Stored { collection: collection.id.clone(), request: step.id.clone() }, options) {
            Ok(outcome) => {
                let mut run = base(outcome.request.method.clone(), outcome.request.url.clone());
                run.status = Some(outcome.response.status);
                run.duration_ms = outcome.response.timings.total_ms;
                run.size_bytes = u64::try_from(outcome.response.size.body_decoded_bytes).unwrap_or(u64::MAX);
                run.tests = outcome.script.tests;
                run.failures = outcome.script.failures;
                (run, outcome.script.next_request)
            }
            Err(err) => {
                let (method, url) = request_head(collection, &step.id);
                let mut run = base(method, url);
                match err {
                    EngineError::Skipped { reason } => run.skipped = Some(reason),
                    other => run.error = Some(other.to_string()),
                }
                (run, NextRequest::Unchanged)
            }
        }
    }

    /// The stored runs of a collection, newest first. Unreadable files are skipped.
    pub fn runs(&self, collection: &Id) -> Result<Vec<RunSummary>> {
        let ids = self.workspace().run_ids(&collection.0)?;
        Ok(ids.iter().filter_map(|id| self.run(collection, id).ok()).map(|result| result.summary()).collect())
    }

    /// One stored run.
    pub fn run(&self, collection: &Id, run_id: &str) -> Result<RunResult> {
        let bytes = self.workspace().load_run(&collection.0, run_id)?;
        serde_json::from_slice(&bytes).map_err(|err| EngineError::Run { message: format!("run `{run_id}` is not readable: {err}") })
    }
}
