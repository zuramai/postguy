//! The Runner tab: pick a collection, environment, requests, iterations, delay and a data
//! file on the left; watch results stream in on the right. View types are mapped by the app
//! from the engine (this crate does not depend on the engine or the runner), like
//! [`crate::response::ResponseData`].

use std::collections::BTreeSet;

use egui::{Align, Layout, RichText, ScrollArea, Ui};
use postguy_model::{TestKind, TestResult, TestStatus};

use crate::response::TestFilter;
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind, ButtonOptions};

/// One request in the collection (or folder) being configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerRequestRow {
    pub id: String,
    pub name: String,
    pub method: String,
    /// Folder path, `Users / Admin`, for the muted second line.
    pub path: String,
    /// False for gRPC and WebSocket requests: the runner skips them.
    pub runnable: bool,
}

/// Everything the panel lists, mapped by the app.
#[derive(Debug, Clone, Default)]
pub struct RunnerView {
    /// `(id, name)` of every collection.
    pub collections: Vec<(String, String)>,
    /// `(id, name)` of every environment.
    pub environments: Vec<(String, String)>,
    /// The chosen collection's requests in run order.
    pub requests: Vec<RunnerRequestRow>,
}

/// One finished request of a run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunRequestView {
    /// 0-based.
    pub iteration: u32,
    pub name: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: u64,
    pub tests: Vec<TestResult>,
    pub error: Option<String>,
    pub skipped: Option<String>,
    /// One line per script failure, `post-response script in request X failed at line N: msg`.
    pub script_failures: Vec<String>,
    /// Unix milliseconds when the send started.
    pub started_ms: i64,
    /// Custom `pg.report.set` fields, in first-set order.
    pub report: Vec<(String, String)>,
    /// The first part of the response body of a failed request.
    pub response_excerpt: Option<String>,
}

impl RunRequestView {
    fn failed(&self) -> bool {
        self.error.is_some() || !self.script_failures.is_empty() || self.tests.iter().any(|test| test.status == TestStatus::Failed)
    }
}

/// A run, streaming or finished.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunResultView {
    pub id: String,
    pub collection: String,
    pub requests: Vec<RunRequestView>,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub duration_ms: u64,
    pub stopped: Option<String>,
}

/// The row a past run shows in a collection's Runs tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummaryView {
    pub id: String,
    pub started_ms: i64,
    pub iterations: usize,
    pub passed: usize,
    pub failed: usize,
    pub duration_ms: u64,
    pub ok: bool,
}

/// Runner tab state (kept by the app).
#[derive(Debug, Clone)]
pub struct RunnerState {
    pub collection: Option<String>,
    pub folder: Option<String>,
    pub environment: Option<String>,
    pub iterations: u64,
    pub delay_ms: u64,
    pub stop_on_error: bool,
    pub data_path: Option<String>,
    /// Column names and the first rows of the data file, as text.
    pub data_preview: Option<(Vec<String>, Vec<Vec<String>>)>,
    pub data_error: Option<String>,
    /// Requests the user unticked.
    pub excluded: BTreeSet<String>,
    pub running: bool,
    /// `(finished, planned)` requests while running.
    pub progress: (usize, usize),
    pub result: Option<RunResultView>,
    pub filter: TestFilter,
    /// Index into `result.requests` of the row whose details show under the list.
    pub selected: Option<usize>,
}

impl Default for RunnerState {
    fn default() -> Self {
        RunnerState {
            collection: None,
            folder: None,
            environment: None,
            iterations: 1,
            delay_ms: 0,
            stop_on_error: false,
            data_path: None,
            data_preview: None,
            data_error: None,
            excluded: BTreeSet::new(),
            running: false,
            progress: (0, 0),
            result: None,
            filter: TestFilter::All,
            selected: None,
        }
    }
}

/// What the user asked the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerAction {
    SelectCollection(String),
    PickData,
    ClearData,
    Run,
    Cancel,
    /// `"csv"`, `"xlsx"`, `"pdf"`, `"html"`, `"json"` or `"junit"`.
    Export(&'static str),
}

const LEFT_WIDTH: f32 = 340.0;

/// Draw the Runner tab.
pub fn runner_panel(ui: &mut Ui, view: &RunnerView, state: &mut RunnerState) -> Option<RunnerAction> {
    let theme = Theme::current(ui.ctx());
    let mut action = None;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(LEFT_WIDTH);
            ScrollArea::vertical().id_salt("runner-config").auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(LEFT_WIDTH - 12.0);
                if let Some(next) = configuration(ui, view, state, &theme) {
                    action = Some(next);
                }
            });
        });
        ui.separator();
        ui.vertical(|ui| {
            if let Some(next) = results(ui, state, &theme) {
                action = Some(next);
            }
        });
    });
    action
}

fn label(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.label(RichText::new(text).small().color(theme.palette.text_muted));
}

fn configuration(ui: &mut Ui, view: &RunnerView, state: &mut RunnerState, theme: &Theme) -> Option<RunnerAction> {
    let mut action = None;
    ui.add_space(8.0);
    let selected_count = view.requests.iter().filter(|row| row.runnable && !state.excluded.contains(&row.id)).count();
    let name = state.collection.as_ref().and_then(|id| view.collections.iter().find(|(candidate, _)| candidate == id)).map(|(_, name)| name.clone());
    let options = ButtonOptions { min_width: Some(LEFT_WIDTH - 8.0), ..ButtonOptions::default() };
    if state.running {
        if widgets::text_button_with(ui, ButtonKind::Danger, None, "Cancel", true, options).clicked() {
            action = Some(RunnerAction::Cancel);
        }
    } else {
        let enabled = name.is_some() && selected_count > 0;
        let text = name.map(|name| format!("Run {name}")).unwrap_or_else(|| "Run".to_string());
        if widgets::text_button_with(ui, ButtonKind::Primary, None, &text, enabled, options).clicked() {
            action = Some(RunnerAction::Run);
        }
    }
    ui.add_space(8.0);
    label(ui, "Collection", theme);
    let groups = [widgets::SelectGroup {
        label: None,
        items: view.collections.iter().map(|(id, name)| widgets::SelectItem::new(id.clone(), name.clone())).collect(),
    }];
    let mut selected = state.collection.clone();
    if widgets::select(ui, "runner-collection", &mut selected, &groups, "Choose a collection")
        && let Some(id) = selected
    {
        state.collection = Some(id.clone());
        state.folder = None;
        state.excluded.clear();
        action = Some(RunnerAction::SelectCollection(id));
    }

    ui.add_space(8.0);
    label(ui, "Environment", theme);
    let mut items = vec![widgets::SelectItem::new("", "No environment")];
    items.extend(view.environments.iter().map(|(id, name)| widgets::SelectItem::new(id.clone(), name.clone())));
    let groups = [widgets::SelectGroup { label: None, items }];
    let mut selected = Some(state.environment.clone().unwrap_or_default());
    if widgets::select(ui, "runner-environment", &mut selected, &groups, "No environment") {
        state.environment = selected.filter(|id| !id.is_empty());
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        label(ui, "Requests", theme);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::text_button(ui, ButtonKind::Ghost, None, "Deselect all", true).clicked() {
                state.excluded = view.requests.iter().filter(|row| row.runnable).map(|row| row.id.clone()).collect();
            }
            if widgets::text_button(ui, ButtonKind::Ghost, None, "Select all", true).clicked() {
                state.excluded.clear();
            }
        });
    });
    ScrollArea::vertical().id_salt("runner-requests").max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
        if view.requests.is_empty() {
            ui.label(RichText::new("This collection has no requests.").color(theme.palette.text_muted));
        }
        for row in &view.requests {
            let mut checked = row.runnable && !state.excluded.contains(&row.id);
            ui.add_enabled_ui(row.runnable, |ui| {
                let text = if row.runnable {
                    format!("{} {}", row.method, row.name)
                } else {
                    format!("{} {} (gRPC/WebSocket: skipped)", row.method, row.name)
                };
                if widgets::checkbox(ui, &mut checked, &text).changed() {
                    if checked {
                        state.excluded.remove(&row.id);
                    } else {
                        state.excluded.insert(row.id.clone());
                    }
                }
            });
            if !row.path.is_empty() {
                ui.label(RichText::new(&row.path).small().color(theme.palette.text_muted));
            }
        }
    });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_sized([90.0, 30.0], egui::Label::new("Iterations"));
        widgets::number_with_unit(ui, "runner-iterations", &mut state.iterations, 1..=10_000, "×", "Iterations");
    });
    ui.horizontal(|ui| {
        ui.add_sized([90.0, 30.0], egui::Label::new("Delay"));
        widgets::number_with_unit(ui, "runner-delay", &mut state.delay_ms, 0..=600_000, "ms", "Delay");
    });
    ui.horizontal(|ui| {
        ui.add_sized([90.0, 30.0], egui::Label::new("Data file"));
        match &state.data_path {
            Some(path) => {
                let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
                ui.label(RichText::new(name).color(theme.palette.text));
                if widgets::text_button(ui, ButtonKind::Ghost, None, "Clear", true).clicked() {
                    action = Some(RunnerAction::ClearData);
                }
            }
            None => {
                if widgets::text_button(ui, ButtonKind::Secondary, None, "Choose…", true).clicked() {
                    action = Some(RunnerAction::PickData);
                }
            }
        }
    });
    if let Some(error) = &state.data_error {
        ui.label(RichText::new(error).small().color(theme.palette.danger));
    }
    if let Some((columns, rows)) = &state.data_preview {
        ScrollArea::horizontal().id_salt("runner-preview").show(ui, |ui| {
            egui::Grid::new("runner-data-preview").striped(true).show(ui, |ui| {
                for column in columns.iter().take(8) {
                    ui.label(RichText::new(column).small().strong());
                }
                ui.end_row();
                for row in rows.iter().take(5) {
                    for value in row.iter().take(8) {
                        ui.label(RichText::new(value).small().color(theme.palette.text_muted));
                    }
                    ui.end_row();
                }
            });
        });
    }
    ui.add_space(8.0);
    let mut stop = state.stop_on_error;
    if widgets::checkbox(ui, &mut stop, "Stop on error").changed() {
        state.stop_on_error = stop;
    }

    action
}

/// One line of the flat results list.
#[derive(Debug, Clone, Copy)]
enum RowRef {
    Iteration(u32),
    Request(usize),
}

const ROW_HEIGHT: f32 = 26.0;
const DETAILS_MAX_HEIGHT: f32 = 240.0;

fn results(ui: &mut Ui, state: &mut RunnerState, theme: &Theme) -> Option<RunnerAction> {
    let mut action = None;
    if state.result.is_none() {
        ui.add_space(8.0);
        ui.label(RichText::new("Choose a collection and press Run. Results appear here.").color(theme.palette.text_muted));
        return None;
    }
    ui.add_space(4.0);
    let running = state.running;
    let progress = state.progress;
    let filter = state.filter;
    let selected = state.selected;
    let mut new_filter = filter;
    let mut new_selected = selected;
    if let Some(result) = &state.result {
        if running {
            let (done, total) = progress;
            ui.label(RichText::new(format!("{done}/{total} requests")).color(theme.palette.text));
            let fraction = if total == 0 { 0.0 } else { (done as f32 / total as f32).clamp(0.0, 1.0) };
            ui.add(egui::ProgressBar::new(fraction).desired_height(2.0));
            ui.add_space(4.0);
        }
        ui.horizontal(|ui| {
            badge(ui, &format!("{} passed", result.passed), theme.palette.success);
            badge(ui, &format!("{} failed", result.failed), if result.failed > 0 { theme.palette.danger } else { theme.palette.text_muted });
            badge(ui, &format!("{} skipped", result.skipped), theme.palette.text_muted);
            badge(ui, &format!("{} ms", result.duration_ms), theme.palette.text_muted);
            if let Some(reason) = &result.stopped {
                badge(ui, reason, theme.palette.warning);
            }
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let failed = result.requests.iter().filter(|run| run.failed()).count();
            let skipped = result.requests.iter().filter(|run| run.skipped.is_some()).count();
            let passed = result.requests.len().saturating_sub(failed).saturating_sub(skipped);
            for (candidate, text, count) in [
                (TestFilter::All, "All", result.requests.len()),
                (TestFilter::Passed, "Passed", passed),
                (TestFilter::Failed, "Failed", failed),
                (TestFilter::Skipped, "Skipped", skipped),
            ] {
                if widgets::sub_tab(ui, text, filter == candidate, Some(count), false).clicked() {
                    new_filter = candidate;
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            label(ui, "Download report", theme);
            let enabled = !running && !result.id.is_empty();
            for (text, format) in [("CSV", "csv"), ("Excel (.xlsx)", "xlsx"), ("PDF", "pdf")] {
                if widgets::text_button(ui, ButtonKind::Primary, None, text, enabled).clicked() {
                    action = Some(RunnerAction::Export(format));
                }
            }
            for (text, format) in [("HTML", "html"), ("JSON", "json"), ("JUnit", "junit")] {
                if widgets::text_button(ui, ButtonKind::Ghost, None, text, enabled).clicked() {
                    action = Some(RunnerAction::Export(format));
                }
            }
        });
        ui.separator();

        let mut rows: Vec<RowRef> = Vec::with_capacity(result.requests.len().saturating_add(1));
        let mut current: Option<u32> = None;
        for (index, run) in result.requests.iter().enumerate() {
            let keep = match filter {
                TestFilter::All => true,
                TestFilter::Passed => !run.failed() && run.skipped.is_none(),
                TestFilter::Failed => run.failed(),
                TestFilter::Skipped => run.skipped.is_some(),
            };
            if !keep {
                continue;
            }
            if current != Some(run.iteration) {
                current = Some(run.iteration);
                rows.push(RowRef::Iteration(run.iteration));
            }
            rows.push(RowRef::Request(index));
        }

        let detail = selected.and_then(|index| result.requests.get(index));
        let details_height = if detail.is_some() { DETAILS_MAX_HEIGHT.min(ui.available_height() * 0.5) } else { 0.0 };
        let list_height = (ui.available_height() - details_height - 8.0).max(ROW_HEIGHT * 2.0);
        ScrollArea::vertical().id_salt("runner-results").max_height(list_height).auto_shrink([false, false]).show_rows(
            ui,
            ROW_HEIGHT,
            rows.len(),
            |ui, range| {
                for row in rows.get(range).unwrap_or(&[]) {
                    match *row {
                        RowRef::Iteration(iteration) => {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), egui::Sense::hover());
                            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
                            child.label(RichText::new(format!("Iteration {}", iteration.saturating_add(1))).strong().color(theme.palette.text));
                        }
                        RowRef::Request(index) => {
                            if let Some(run) = result.requests.get(index)
                                && request_row(ui, index, run, selected == Some(index), theme)
                            {
                                new_selected = Some(index);
                            }
                        }
                    }
                }
            },
        );
        if let Some(run) = detail {
            ui.separator();
            ScrollArea::vertical().id_salt("runner-details").max_height(DETAILS_MAX_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
                details(ui, run, theme);
            });
        }
    }
    state.filter = new_filter;
    state.selected = new_selected;
    action
}

fn badge(ui: &mut Ui, text: &str, color: egui::Color32) {
    widgets::badge(ui, text, color, color.gamma_multiply(0.12));
}

/// `(PASS|FAIL|SKIP|ERROR, colour)` for a request line.
fn run_badge(run: &RunRequestView, theme: &Theme) -> (&'static str, egui::Color32) {
    if run.skipped.is_some() {
        ("SKIP", theme.palette.text_muted)
    } else if run.error.is_some() {
        ("ERROR", theme.palette.danger)
    } else if run.failed() {
        ("FAIL", theme.palette.danger)
    } else {
        ("PASS", theme.palette.success)
    }
}

/// One fixed-height line; returns true when it was clicked.
fn request_row(ui: &mut Ui, index: usize, run: &RunRequestView, selected: bool, theme: &Theme) -> bool {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), egui::Sense::hover());
    if selected {
        ui.painter().rect_filled(rect, 3.0, theme.palette.accent.gamma_multiply(0.12));
    }
    let (text, color) = run_badge(run, theme);
    let status = run.status.map(|status| status.to_string()).unwrap_or_else(|| "-".to_string());
    let passed = run.tests.iter().filter(|test| test.status == TestStatus::Passed).count();
    let name_color = if run.failed() {
        theme.palette.danger
    } else if run.skipped.is_some() {
        theme.palette.text_muted
    } else {
        theme.palette.text
    };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(4.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    badge(&mut child, text, color);
    child.add_sized([(rect.width() - 330.0).max(80.0), ROW_HEIGHT], egui::Label::new(RichText::new(&run.name).color(name_color)).truncate());
    for (value, width) in [
        (run.method.clone(), 56.0),
        (status, 36.0),
        (if run.skipped.is_some() { String::new() } else { format!("{} ms", run.duration_ms) }, 64.0),
        (format!("{passed}/{} tests", run.tests.len()), 80.0),
    ] {
        child.add_sized([width, ROW_HEIGHT], egui::Label::new(RichText::new(value).small().color(theme.palette.text_muted)).truncate());
    }
    let response = ui.interact(rect, ui.id().with(("runner-row", index)), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Result {} {}", run.method, run.name)));
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// A test or assertion line: result badge, an "assert" badge for assertions, the name and, for
/// a failure, the message.
fn result_row(ui: &mut Ui, test: &TestResult, theme: &Theme) {
    let (text, color) = match test.status {
        TestStatus::Passed => ("PASS", theme.palette.success),
        TestStatus::Failed => ("FAIL", theme.palette.danger),
        TestStatus::Skipped => ("SKIP", theme.palette.text_muted),
    };
    ui.horizontal_top(|ui| {
        badge(ui, text, color);
        if test.kind == TestKind::Assertion {
            badge(ui, "assert", theme.palette.accent);
        }
        ui.vertical(|ui| {
            ui.label(RichText::new(&test.name).color(theme.palette.text));
            if let (TestStatus::Failed, Some(message)) = (test.status, &test.message) {
                ui.label(RichText::new(message).small().color(theme.palette.danger));
            }
        });
    });
}

fn details(ui: &mut Ui, run: &RunRequestView, theme: &Theme) {
    let muted = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).small().color(theme.palette.text_muted));
    };
    muted(ui, &run.url);
    if let Some(reason) = &run.skipped {
        muted(ui, reason);
    }
    if let Some(error) = &run.error {
        ui.label(RichText::new(error).small().color(theme.palette.danger));
    }
    for failure in &run.script_failures {
        ui.label(RichText::new(failure).small().color(theme.palette.danger));
    }
    for test in &run.tests {
        result_row(ui, test, theme);
    }
    if !run.report.is_empty() {
        ui.add_space(4.0);
        label(ui, "Report fields", theme);
        for (key, value) in &run.report {
            ui.label(RichText::new(format!("{key}: {value}")).small().color(theme.palette.text));
        }
    }
    if let Some(excerpt) = &run.response_excerpt {
        ui.add_space(4.0);
        label(ui, "Response excerpt", theme);
        ui.label(RichText::new(excerpt).small().monospace().color(theme.palette.text));
    }
}
