//! The Runner tab's app glue: view mapping, the run thread, data-file and export dialogs, and
//! the Runs tab's data. The engine does the work; this module only moves results between the
//! worker threads and the egui state.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::mpsc;

use postguy_engine::{Cancel, DataSet, RunConfig, RunEvent, RunResult};
use postguy_model::{Collection, Id, Item, RequestKind};
use postguy_ui_egui::runner::{RunRequestView, RunResultView, RunSummaryView, RunnerAction, RunnerRequestRow, RunnerState, RunnerView};
use postguy_ui_egui::tabs::TabDoc;

use crate::{App, RepaintWake};

type Picker = Pin<Box<dyn Future<Output = Option<PathBuf>>>>;

/// `(sidebar revision, collection, folder, rows)`: the request rows for one selection.
type RowsCache = (u64, Option<String>, Option<String>, Vec<RunnerRequestRow>);

/// `(sidebar revision, runs version, collection, runs)`.
type RunsCache = (u64, u64, String, Vec<RunSummaryView>);

pub(crate) enum RunnerMsg {
    Event(Box<RunEvent>),
    Done(Box<Result<RunResult, String>>),
}

pub(crate) struct RunnerJob {
    receiver: mpsc::Receiver<RunnerMsg>,
    cancel: Cancel,
}

/// Everything the Runner tab keeps besides its egui state.
#[derive(Default)]
pub(crate) struct RunnerUi {
    pub state: RunnerState,
    job: Option<RunnerJob>,
    data: Option<DataSet>,
    data_picker: Option<Picker>,
    data_job: Option<mpsc::Receiver<Result<(String, DataSet), String>>>,
    export_picker: Option<(Picker, &'static str)>,
    export_job: Option<mpsc::Receiver<Result<String, String>>>,
    rows: Option<RowsCache>,
    runs: Option<RunsCache>,
    /// Bumped when a run finishes, so the Runs tab reloads.
    runs_version: u64,
}

fn method_labels(collection: &Collection) -> std::collections::BTreeMap<String, String> {
    let mut labels = std::collections::BTreeMap::new();
    let mut stack: Vec<&Item> = collection.items.iter().collect();
    let mut visited = 0usize;
    while let Some(item) = stack.pop() {
        visited += 1;
        if visited > 200_000 {
            break;
        }
        match item {
            Item::Folder(folder) => stack.extend(folder.items.iter()),
            Item::Request(request) => {
                let label = match &request.kind {
                    RequestKind::Http(http) => http.method.short_label().to_string(),
                    RequestKind::Grpc(_) => "gRPC".to_string(),
                    RequestKind::WebSocket(_) => "WS".to_string(),
                    _ => String::new(),
                };
                labels.insert(request.id.0.clone(), label);
            }
        }
    }
    labels
}

fn view_of_result(result: &RunResult) -> RunResultView {
    let requests =
        result.iterations.iter().flat_map(|iteration| iteration.requests.iter().map(move |run| request_view(iteration.index, run))).collect();
    let totals = &result.totals;
    RunResultView {
        id: result.id.clone(),
        collection: result.collection_name.clone(),
        requests,
        passed: totals.tests_passed,
        failed: totals
            .tests_failed
            .saturating_add(totals.script_failures)
            .saturating_add(result.iterations.iter().flat_map(|i| &i.requests).filter(|run| run.error.is_some()).count()),
        skipped: totals.tests_skipped.saturating_add(totals.skipped_requests),
        duration_ms: result.duration_ms(),
        stopped: result.stopped.clone(),
    }
}

fn request_view(iteration: u32, run: &postguy_runner::RequestRun) -> RunRequestView {
    RunRequestView {
        iteration,
        name: run.name.clone(),
        method: run.method.clone(),
        url: run.url.clone(),
        status: run.status,
        duration_ms: run.duration_ms,
        tests: run.tests.clone(),
        error: run.error.clone().or_else(|| run.failures.first().map(|failure| format!("{} script: {}", failure.phase.label(), failure.message))),
        skipped: run.skipped.clone(),
    }
}

impl App {
    /// Open (or focus) the Runner tab, optionally preselecting a collection and folder.
    pub(crate) fn open_runner(&mut self, collection: Option<&Id>, folder: Option<&Id>) {
        if let Some(collection) = collection {
            let state = &mut self.runner.state;
            if !state.running {
                state.collection = Some(collection.0.clone());
                state.folder = folder.map(|id| id.0.clone());
                state.excluded.clear();
                state.result = None;
            }
            self.runner.rows = None;
        }
        self.shell.tabs.open(TabDoc::Runner, "Runner");
    }

    /// The collections, environments and request rows the panel lists.
    pub(crate) fn runner_view(&mut self) -> RunnerView {
        let data = &self.shell.sidebar_data;
        let collections = data.collections.iter().map(|collection| (collection.id.0.clone(), collection.name.clone())).collect();
        let environments = data.environments.iter().map(|environment| (environment.id.0.clone(), environment.name.clone())).collect();
        let key = (data.revision, self.runner.state.collection.clone(), self.runner.state.folder.clone());
        let fresh = matches!(&self.runner.rows, Some((revision, collection, folder, _)) if (*revision, collection.clone(), folder.clone()) == key);
        if !fresh {
            let rows = self.runner_rows();
            self.runner.rows = Some((key.0, key.1, key.2, rows));
        }
        let requests = self.runner.rows.as_ref().map(|(_, _, _, rows)| rows.clone()).unwrap_or_default();
        RunnerView { collections, environments, requests }
    }

    fn runner_rows(&self) -> Vec<RunnerRequestRow> {
        let Some(id) = &self.runner.state.collection else { return Vec::new() };
        let Some(collection) = self.shell.sidebar_data.collections.iter().find(|collection| collection.id.0 == *id) else { return Vec::new() };
        let folder = self.runner.state.folder.clone().map(Id);
        let Ok(planned) = postguy_runner::plan(collection, folder.as_ref(), None) else { return Vec::new() };
        let labels = method_labels(collection);
        planned
            .into_iter()
            .map(|step| RunnerRequestRow {
                method: labels.get(&step.id.0).cloned().unwrap_or_default(),
                id: step.id.0,
                name: step.name,
                path: step.path.join(" / "),
                runnable: step.protocol == "http",
            })
            .collect()
    }

    /// The Runs tab rows of a collection, cached until a run finishes or the sidebar reloads.
    pub(crate) fn runs_for(&mut self, collection: &str) -> Vec<RunSummaryView> {
        let key = (self.revision, self.runner.runs_version);
        if let Some((revision, version, id, runs)) = &self.runner.runs
            && (*revision, *version) == key
            && id == collection
        {
            return runs.clone();
        }
        let runs: Vec<RunSummaryView> = self
            .engine
            .as_ref()
            .and_then(|engine| engine.runs(&Id(collection.to_string())).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|run| RunSummaryView {
                id: run.id,
                started_ms: run.started_ms,
                iterations: run.iterations,
                passed: run.totals.tests_passed,
                failed: run.totals.tests_failed.saturating_add(run.totals.script_failures),
                duration_ms: u64::try_from(run.finished_ms.saturating_sub(run.started_ms)).unwrap_or(0),
                ok: run.passed,
            })
            .collect();
        self.runner.runs = Some((key.0, key.1, collection.to_string(), runs.clone()));
        runs
    }

    /// Open a stored run in the Runner tab.
    pub(crate) fn open_stored_run(&mut self, collection: &Id, run_id: &str) {
        let Some(engine) = self.engine.clone() else { return };
        match engine.run(collection, run_id) {
            Ok(result) => {
                if !self.runner.state.running {
                    self.runner.state.collection = Some(collection.0.clone());
                    self.runner.state.folder = None;
                    self.runner.state.result = Some(view_of_result(&result));
                    self.runner.rows = None;
                }
                self.shell.tabs.open(TabDoc::Runner, "Runner");
            }
            Err(err) => self.shell.state.status = format!("Could not open the run: {err}"),
        }
    }

    pub(crate) fn apply_runner(&mut self, action: RunnerAction) {
        match action {
            RunnerAction::SelectCollection(_) => self.runner.rows = None,
            RunnerAction::PickData => {
                if self.runner.data_picker.is_none() && self.runner.data_job.is_none() {
                    self.runner.data_picker = Some(postguy_platform::pick_data_file());
                }
            }
            RunnerAction::ClearData => {
                self.runner.data = None;
                let state = &mut self.runner.state;
                state.data_path = None;
                state.data_preview = None;
                state.data_error = None;
            }
            RunnerAction::Run => self.start_run(),
            RunnerAction::Cancel => {
                if let Some(job) = &self.runner.job {
                    job.cancel.cancel();
                    self.shell.state.status = "Cancelling the run…".into();
                }
            }
            RunnerAction::Export(format) => {
                if self.runner.export_picker.is_none() && self.runner.export_job.is_none() {
                    let name = self.runner.state.result.as_ref().map(|result| result.collection.clone()).unwrap_or_else(|| "run".to_string());
                    let extension = if format == "junit" { "xml" } else { format };
                    self.runner.export_picker = Some((postguy_platform::save_export(format!("{name}-run.{extension}"), extension), format));
                }
            }
        }
    }

    fn start_run(&mut self) {
        let Some(engine) = self.engine.clone() else { return };
        if self.runner.job.is_some() {
            return;
        }
        let Some(collection) = self.runner.state.collection.clone() else { return };
        let rows = self.runner.rows.as_ref().map(|(_, _, _, rows)| rows.clone()).unwrap_or_default();
        let selected: Vec<Id> =
            rows.iter().filter(|row| row.runnable && !self.runner.state.excluded.contains(&row.id)).map(|row| Id(row.id.clone())).collect();
        if selected.is_empty() {
            self.shell.state.status = "Select at least one request to run".into();
            return;
        }
        let state = &self.runner.state;
        let data_rows = self.runner.data.as_ref().map(|set| set.rows.len());
        let iterations = if data_rows.is_some() && state.iterations <= 1 { None } else { Some(u32::try_from(state.iterations).unwrap_or(u32::MAX)) };
        let planned_iterations = iterations.map(u64::from).or_else(|| data_rows.and_then(|rows| u64::try_from(rows).ok())).unwrap_or(1);
        let config = RunConfig {
            collection: Id(collection),
            folder: state.folder.clone().map(Id),
            environment: state.environment.clone().map(Id),
            iterations,
            delay_ms: state.delay_ms,
            stop_on_error: state.stop_on_error,
            requests: (!state.excluded.is_empty()).then(|| selected.clone()),
        };
        let name = self
            .shell
            .sidebar_data
            .collections
            .iter()
            .find(|candidate| candidate.id == config.collection)
            .map(|candidate| candidate.name.clone())
            .unwrap_or_default();
        let data = self.runner.data.clone();
        let cancel = Cancel::new();
        let thread_cancel = cancel.clone();
        let (sender, receiver) = mpsc::channel();
        let total = usize::try_from(planned_iterations).unwrap_or(usize::MAX).saturating_mul(if config.requests.is_some() {
            selected.len()
        } else {
            rows.len()
        });
        let spawned = std::thread::Builder::new().name("postguy-run".into()).spawn(move || {
            let events = sender.clone();
            let result = engine.run_collection(&config, data, &thread_cancel, &mut |event| {
                let _ = events.send(RunnerMsg::Event(Box::new(event.clone())));
            });
            let _ = sender.send(RunnerMsg::Done(Box::new(result.map_err(|err| err.to_string()))));
        });
        if spawned.is_err() {
            self.shell.state.status = "Could not start the run thread".into();
            return;
        }
        self.runner.job = Some(RunnerJob { receiver, cancel });
        let state = &mut self.runner.state;
        state.running = true;
        state.progress = (0, total);
        state.result = Some(RunResultView { collection: name, ..RunResultView::default() });
        self.shell.state.status = "Running…".into();
    }

    /// Drain the run thread, the data-file loader and the dialogs. Repaints only while a run
    /// is in flight (no permanent loop).
    pub(crate) fn poll_runner(&mut self, ctx: &egui::Context) {
        self.poll_runner_job(ctx);
        self.poll_runner_pickers(ctx);
    }

    fn poll_runner_job(&mut self, ctx: &egui::Context) {
        let Some(job) = self.runner.job.take() else { return };
        let mut finished = false;
        loop {
            match job.receiver.try_recv() {
                Ok(RunnerMsg::Event(event)) => {
                    if let RunEvent::Request { iteration, run } = *event {
                        let state = &mut self.runner.state;
                        if let Some(result) = &mut state.result {
                            let view = request_view(iteration, &run);
                            result.passed += view.tests.iter().filter(|test| test.status == postguy_model::TestStatus::Passed).count();
                            result.failed += view.tests.iter().filter(|test| test.status == postguy_model::TestStatus::Failed).count()
                                + usize::from(view.error.is_some());
                            result.skipped += usize::from(view.skipped.is_some());
                            result.requests.push(view);
                        }
                        state.progress.0 += 1;
                    }
                }
                Ok(RunnerMsg::Done(result)) => {
                    finished = true;
                    self.runner.state.running = false;
                    match *result {
                        Ok(result) => {
                            self.shell.state.status =
                                format!("Run finished: {} passed, {} failed", result.totals.tests_passed, result.totals.tests_failed);
                            self.runner.state.result = Some(view_of_result(&result));
                        }
                        Err(err) => {
                            self.shell.state.status = format!("Run failed: {err}");
                            if let Some(result) = &mut self.runner.state.result {
                                result.stopped = Some(err);
                            }
                        }
                    }
                    self.runner.runs_version = self.runner.runs_version.wrapping_add(1);
                    self.refresh();
                    self.reload_variable_editors();
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    finished = true;
                    self.runner.state.running = false;
                    self.shell.state.status = "The run worker stopped without a result".into();
                    break;
                }
            }
        }
        if finished {
            return;
        }
        self.runner.job = Some(job);
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn poll_runner_pickers(&mut self, ctx: &egui::Context) {
        let waker = std::task::Waker::from(std::sync::Arc::new(RepaintWake(ctx.clone())));
        let mut task = std::task::Context::from_waker(&waker);
        if let Some(mut picker) = self.runner.data_picker.take() {
            match picker.as_mut().poll(&mut task) {
                std::task::Poll::Pending => self.runner.data_picker = Some(picker),
                std::task::Poll::Ready(Some(path)) => {
                    let (sender, receiver) = mpsc::channel();
                    self.runner.data_job = Some(receiver);
                    let ctx = ctx.clone();
                    let spawned = std::thread::Builder::new().name("postguy-run-data".into()).spawn(move || {
                        let loaded =
                            postguy_engine::load_data_file(&path).map(|set| (path.display().to_string(), set)).map_err(|err| err.to_string());
                        let _ = sender.send(loaded);
                        ctx.request_repaint();
                    });
                    if spawned.is_err() {
                        self.runner.data_job = None;
                    }
                }
                std::task::Poll::Ready(None) => {}
            }
        }
        if let Some(receiver) = self.runner.data_job.take() {
            match receiver.try_recv() {
                Ok(Ok((path, set))) => {
                    let state = &mut self.runner.state;
                    let columns: Vec<String> = set.rows.first().map(|row| row.iter().map(|(name, _)| name.clone()).collect()).unwrap_or_default();
                    let rows = set
                        .rows
                        .iter()
                        .take(5)
                        .map(|row| row.iter().map(|(_, value)| value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string())).collect())
                        .collect();
                    state.data_path = Some(path);
                    state.data_preview = Some((columns, rows));
                    state.data_error = None;
                    self.runner.data = Some(set);
                }
                Ok(Err(err)) => {
                    self.runner.data = None;
                    let state = &mut self.runner.state;
                    state.data_path = None;
                    state.data_preview = None;
                    state.data_error = Some(err);
                }
                Err(mpsc::TryRecvError::Empty) => self.runner.data_job = Some(receiver),
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if let Some((mut picker, format)) = self.runner.export_picker.take() {
            match picker.as_mut().poll(&mut task) {
                std::task::Poll::Pending => self.runner.export_picker = Some((picker, format)),
                std::task::Poll::Ready(Some(path)) => self.start_run_export(path, format, ctx),
                std::task::Poll::Ready(None) => {}
            }
        }
        if let Some(receiver) = self.runner.export_job.take() {
            match receiver.try_recv() {
                Ok(Ok(message)) => self.shell.state.status = message,
                Ok(Err(err)) => self.shell.state.status = format!("Export failed: {err}"),
                Err(mpsc::TryRecvError::Empty) => self.runner.export_job = Some(receiver),
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
    }

    fn start_run_export(&mut self, path: PathBuf, format: &'static str, ctx: &egui::Context) {
        let Some(engine) = self.engine.clone() else { return };
        let Some(collection) = self.runner.state.collection.clone() else { return };
        let Some(run) = self.runner.state.result.as_ref().map(|result| result.id.clone()).filter(|id| !id.is_empty()) else { return };
        let (sender, receiver) = mpsc::channel();
        self.runner.export_job = Some(receiver);
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("postguy-run-export".into()).spawn(move || {
            let outcome = engine.run(&Id(collection), &run).map_err(|err| err.to_string()).and_then(|result| {
                let bytes = match format {
                    "junit" => postguy_runner::report::junit(&result).into_bytes(),
                    "html" => postguy_runner::report::html(&result).into_bytes(),
                    _ => serde_json::to_vec_pretty(&postguy_runner::report::json(&result)).map_err(|err| err.to_string())?,
                };
                postguy_util::atomic_write(&path, &bytes).map_err(|err| err.to_string())?;
                Ok(format!("Run exported to {}", path.display()))
            });
            let _ = sender.send(outcome);
            ctx.request_repaint();
        });
        if spawned.is_err() {
            self.runner.export_job = None;
        }
    }
}
