//! The Runner tab: pick a collection, environment, requests, iterations, delay and a data
//! file on the left; watch results stream in on the right. View types are mapped by the app
//! from the engine (this crate does not depend on the engine or the runner), like
//! [`crate::response::ResponseData`].

use std::collections::BTreeSet;

use egui::{Align, Layout, RichText, ScrollArea, Ui};
use postguy_model::{TestResult, TestStatus};

use crate::response::{TestFilter, test_row};
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
}

impl RunRequestView {
    fn failed(&self) -> bool {
        self.error.is_some() || self.tests.iter().any(|test| test.status == TestStatus::Failed)
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
    /// `"json"`, `"junit"` or `"html"`.
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

fn results(ui: &mut Ui, state: &mut RunnerState, theme: &Theme) -> Option<RunnerAction> {
    let mut action = None;
    let Some(result) = state.result.clone() else {
        ui.add_space(8.0);
        ui.label(RichText::new("Choose a collection and press Run. Results appear here.").color(theme.palette.text_muted));
        return None;
    };
    ui.add_space(4.0);
    if state.running {
        let (done, total) = state.progress;
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
        for (filter, text, count) in [
            (TestFilter::All, "All", result.requests.len()),
            (TestFilter::Passed, "Passed", passed),
            (TestFilter::Failed, "Failed", failed),
            (TestFilter::Skipped, "Skipped", skipped),
        ] {
            if widgets::sub_tab(ui, text, state.filter == filter, Some(count), false).clicked() {
                state.filter = filter;
            }
        }
    });
    ui.horizontal(|ui| {
        label(ui, "Export", theme);
        for (text, format) in [("JSON", "json"), ("JUnit", "junit"), ("HTML", "html")] {
            if widgets::text_button(ui, ButtonKind::Ghost, None, text, !state.running && !result.id.is_empty()).clicked() {
                action = Some(RunnerAction::Export(format));
            }
        }
    });
    ui.separator();
    ScrollArea::vertical().id_salt("runner-results").auto_shrink([false, false]).show(ui, |ui| {
        let mut current: Option<u32> = None;
        for (index, run) in result.requests.iter().enumerate() {
            let keep = match state.filter {
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
                ui.add_space(6.0);
                ui.label(RichText::new(format!("Iteration {}", run.iteration.saturating_add(1))).strong().color(theme.palette.text));
            }
            request_row(ui, index, run, theme);
        }
    });
    action
}

fn badge(ui: &mut Ui, text: &str, color: egui::Color32) {
    widgets::badge(ui, text, color, color.gamma_multiply(0.12));
}

fn request_row(ui: &mut Ui, index: usize, run: &RunRequestView, theme: &Theme) {
    let status = run.status.map(|status| status.to_string()).unwrap_or_else(|| "-".to_string());
    let title = if run.skipped.is_some() {
        format!("{} {} (skipped)", run.method, run.name)
    } else {
        format!("{} {} - {status} - {} ms", run.method, run.name, run.duration_ms)
    };
    let color = if run.failed() {
        theme.palette.danger
    } else if run.skipped.is_some() {
        theme.palette.text_muted
    } else {
        theme.palette.text
    };
    egui::CollapsingHeader::new(RichText::new(title).color(color)).id_salt(("runner-row", index)).show(ui, |ui| {
        ui.label(RichText::new(&run.url).small().color(theme.palette.text_muted));
        if let Some(reason) = &run.skipped {
            ui.label(RichText::new(reason).small().color(theme.palette.text_muted));
        }
        if let Some(error) = &run.error {
            ui.label(RichText::new(error).small().color(theme.palette.danger));
        }
        for test in &run.tests {
            test_row(ui, test, theme);
        }
    });
}
