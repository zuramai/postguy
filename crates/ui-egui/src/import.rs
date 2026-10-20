//! The import dialog (PROMPT.md 5.7, M4.1): one box for Postman JSON or cURL text, file/folder
//! pickers and a drop hint, an explicit Replace option, and the per-file ImportReport view.
//! Parsing and workspace writes happen outside the UI thread; this widget is data/actions only.

use egui::{Align, Layout, RichText, ScrollArea};
use postguy_interop::ImportReport;

use crate::theme::Theme;
use crate::widgets;

/// One imported document (or one failed file) in the report.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportItemView {
    pub source: String,
    /// "collection", "environment" or "globals".
    pub kind: String,
    pub name: String,
    pub id: Option<String>,
    pub report: ImportReport,
    pub error: Option<String>,
}

/// The aggregate result of one dialog import run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportSummary {
    pub items: Vec<ImportItemView>,
    pub problems: Vec<String>,
}

/// Dialog state owned by the app.
#[derive(Debug, Clone, Default)]
pub struct ImportState {
    pub open: bool,
    pub text: String,
    pub replace: bool,
    pub busy: bool,
    pub summary: Option<ImportSummary>,
    pub error: Option<String>,
}

/// What the dialog asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportAction {
    ImportText { text: String, replace: bool },
    PickFiles { folder: bool },
    Open { index: usize },
}

/// Draw the modal while `state.open` is set.
pub fn import_dialog(ctx: &egui::Context, state: &mut ImportState) -> Option<ImportAction> {
    let theme = Theme::current(ctx);
    let mut action = None;
    let open = state.open;
    let result = widgets::modal(ctx, "import-dialog", "Import", &mut state.open, |ui| {
        ui.set_min_width(560.0);
        ui.label(RichText::new("Postman collection v2.0/v2.1, environment, globals, or a cURL command.").small().color(theme.palette.text_muted));
        ui.add_space(6.0);
        ui.add(
            egui::TextEdit::multiline(&mut state.text).desired_rows(8).desired_width(f32::INFINITY).hint_text("Paste JSON or a cURL command here…"),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let can_import = !state.text.trim().is_empty() && !state.busy;
            if ui.add_enabled(can_import, egui::Button::new("Import text")).clicked() {
                action = Some(ImportAction::ImportText { text: state.text.clone(), replace: state.replace });
            }
            if ui.add_enabled(!state.busy, egui::Button::new("Choose files…")).clicked() {
                action = Some(ImportAction::PickFiles { folder: false });
            }
            if ui.add_enabled(!state.busy, egui::Button::new("Choose folder…")).clicked() {
                action = Some(ImportAction::PickFiles { folder: true });
            }
            let mut replace = state.replace;
            if crate::widgets::checkbox(ui, &mut replace, "Replace existing").changed() {
                state.replace = replace;
            }
        });
        ui.label(
            RichText::new("Or drop files or folders anywhere in the window. Nothing is fetched from the network.")
                .small()
                .color(theme.palette.text_muted),
        );
        if state.busy {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Importing…");
            });
        }
        if let Some(error) = &state.error {
            ui.add_space(8.0);
            ui.label(RichText::new(error).color(theme.palette.danger));
        }
        if let Some(summary) = &state.summary {
            ui.add_space(8.0);
            ui.separator();
            let (mut collections, mut environments, mut globals, mut requests, mut folders, mut examples) = (0, 0, 0, 0, 0, 0);
            for item in &summary.items {
                collections += item.report.collections;
                environments += item.report.environments;
                globals += item.report.globals;
                requests += item.report.requests;
                folders += item.report.folders;
                examples += item.report.examples;
            }
            ui.label(
                RichText::new(format!(
                    "Imported {collections} collection(s), {environments} environment(s), {globals} globals · {requests} requests, {folders} folders, {examples} examples"
                ))
                .strong(),
            );
            ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for (index, item) in summary.items.iter().enumerate() {
                    ui.separator();
                    if let Some(error) = &item.error {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&item.source).monospace());
                            ui.label(RichText::new(error).color(theme.palette.danger));
                        });
                        continue;
                    }
                    ui.horizontal(|ui| {
                        let kind = match item.kind.as_str() {
                            "collection" => "Collection",
                            "environment" => "Environment",
                            "globals" => "Globals",
                            other => other,
                        };
                        ui.label(RichText::new(item.name.clone()).strong());
                        ui.label(RichText::new(kind).small().color(theme.palette.text_muted));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(egui::Button::new("Open").frame(false)).clicked() {
                                action = Some(ImportAction::Open { index });
                            }
                        });
                    });
                    if !item.report.warnings.is_empty() {
                        for warning in item.report.warnings.iter().take(8) {
                            ui.label(RichText::new(format!("warning: {warning}")).small().color(theme.palette.warning));
                        }
                        if item.report.warnings.len() > 8 {
                            ui.label(
                                RichText::new(format!("… {} more warning(s)", item.report.warnings.len() - 8))
                                    .small()
                                    .color(theme.palette.text_muted),
                            );
                        }
                    }
                    if !item.report.dropped.is_empty() {
                        for dropped in item.report.dropped.iter().take(5) {
                            ui.label(RichText::new(format!("dropped: {dropped}")).small().color(theme.palette.danger));
                        }
                    }
                }
                if !summary.problems.is_empty() {
                    ui.separator();
                    for problem in &summary.problems {
                        ui.label(RichText::new(problem).small().color(theme.palette.danger));
                    }
                }
            });
        }
    });
    if !open {
        return None;
    }
    if result.is_none() && !state.open {
        return None;
    }
    action
}

/// Open the dialog with a fresh run state, keeping the Replace preference.
pub fn open(state: &mut ImportState) {
    state.open = true;
    state.busy = false;
    state.error = None;
    state.summary = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_closed_and_empty() {
        let state = ImportState::default();
        assert!(!state.open && state.text.is_empty() && !state.busy);
    }

    #[test]
    fn opening_clears_a_previous_run_but_keeps_replace() {
        let mut state = ImportState { replace: true, text: "old".into(), busy: true, error: Some("x".into()), ..ImportState::default() };
        open(&mut state);
        assert!(state.open && !state.busy && state.error.is_none());
        assert_eq!(state.text, "old", "the pasted text is intentionally kept for a retry");
        assert!(state.replace);
    }
}
