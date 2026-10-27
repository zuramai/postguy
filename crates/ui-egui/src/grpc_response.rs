//! The gRPC response pane: status strip, the response messages as JSON, trailing metadata and
//! the test results of the Test tab and the scripts.
//!
//! Like the HTTP viewer it takes plain data mapped by the app (this crate does not depend on
//! the engine); the pretty-printed body is built off the UI thread by the caller.

use std::sync::Arc;

use egui::{RichText, ScrollArea, Ui};
use postguy_model::{ScriptFailure, TestResult, TestStatus};

use crate::response::{TestFilter, test_tab_label, tests_view};
use crate::theme::Theme;
use crate::widgets::{self, CodeLanguage, LargeTextView};

/// Everything the pane shows for one finished invoke.
#[derive(Debug, Clone)]
pub struct GrpcResponseData {
    pub status_code: i32,
    pub status_name: String,
    pub status_message: String,
    pub elapsed_ms: u64,
    pub metadata: Vec<(String, String)>,
    pub message_count: usize,
    /// True when the method streams its output (the body is then a JSON array).
    pub streaming: bool,
    /// The response as pretty JSON, built off-thread.
    pub body: Arc<str>,
    pub tests: Vec<TestResult>,
    pub script_failures: Vec<ScriptFailure>,
    pub warnings: Vec<String>,
}

/// Which tab of the pane is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrpcResponseTab {
    #[default]
    Response,
    Metadata,
    Tests,
}

/// Per-tab view state of the pane.
#[derive(Debug, Clone, Default)]
pub struct GrpcResponseState {
    pub tab: GrpcResponseTab,
    pub test_filter: TestFilter,
}

/// Draw the response pane for `data`.
pub fn grpc_response_view(ui: &mut Ui, data: &GrpcResponseData, state: &mut GrpcResponseState) {
    let theme = Theme::current(ui.ctx());
    let palette = theme.palette;
    ui.horizontal(|ui| {
        let color = if data.status_code == 0 { palette.success } else { palette.danger };
        widgets::badge(ui, &format!("{} {}", data.status_code, data.status_name), color, color.gamma_multiply(0.15));
        ui.label(RichText::new(format!("{} ms", data.elapsed_ms)).small().color(palette.text_muted));
        if data.streaming {
            ui.label(RichText::new(format!("{} messages", data.message_count)).small().color(palette.text_muted));
        }
        ui.add_space(8.0);
        let tests_label = test_tab_label(&data.tests);
        let failed = data.tests.iter().any(|test| test.status == TestStatus::Failed);
        for (tab, label, count, dot) in [
            (GrpcResponseTab::Response, "Response", None, false),
            (GrpcResponseTab::Metadata, "Metadata", Some(data.metadata.len()), false),
            (GrpcResponseTab::Tests, tests_label.as_str(), None, failed),
        ] {
            if widgets::sub_tab(ui, label, state.tab == tab, count, dot).clicked() {
                state.tab = tab;
            }
        }
    });
    for warning in &data.warnings {
        ui.label(RichText::new(warning).small().color(palette.warning));
    }
    ui.separator();
    match state.tab {
        GrpcResponseTab::Response => {
            if data.status_code != 0 {
                egui::Frame::new().stroke(egui::Stroke::new(1.0, palette.danger)).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                    ui.label(RichText::new(&data.status_message).color(palette.danger));
                });
                ui.add_space(4.0);
            }
            ScrollArea::both().id_salt("grpc-response-body").auto_shrink([false, false]).show(ui, |ui| {
                LargeTextView::new(data.body.clone()).id(egui::Id::new("grpc-response-text")).language(CodeLanguage::Json).wrap(true).show(ui);
            });
        }
        GrpcResponseTab::Metadata => {
            if data.metadata.is_empty() {
                ui.label(RichText::new("No metadata.").color(palette.text_muted));
            } else {
                ScrollArea::vertical().id_salt("grpc-response-metadata").auto_shrink([false, false]).show(ui, |ui| {
                    egui::Grid::new("grpc-response-metadata-grid").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                        for (key, value) in &data.metadata {
                            ui.label(RichText::new(key).strong());
                            ui.label(RichText::new(value).monospace());
                            ui.end_row();
                        }
                    });
                });
            }
        }
        GrpcResponseTab::Tests => tests_view(
            ui,
            &data.tests,
            &data.script_failures,
            &mut state.test_filter,
            "No tests ran. Add tests in the Test tab or pm.test(...) in a script.",
            &theme,
        ),
    }
}
