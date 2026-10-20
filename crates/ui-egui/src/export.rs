//! Export dialog (PROMPT.md 5.7, M4.3): choose the shape for a collection, environment, globals
//! or the whole workspace, opt into secret values where the product allows it, then hand off to
//! the app's native save dialog. All file I/O and serialisation happen outside this widget.

use egui::{Align, Layout, RichText};

use crate::theme::Theme;
use crate::widgets;

/// What the dialog exports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportTarget {
    Collection { id: String, name: String },
    Environment { id: String, name: String },
    Globals,
    Workspace,
}

impl ExportTarget {
    fn label(&self) -> &'static str {
        match self {
            ExportTarget::Collection { .. } => "Collection",
            ExportTarget::Environment { .. } => "Environment",
            ExportTarget::Globals => "Globals",
            ExportTarget::Workspace => "All workspace data",
        }
    }

    fn name(&self) -> &str {
        match self {
            ExportTarget::Collection { name, .. } | ExportTarget::Environment { name, .. } => name,
            ExportTarget::Globals => "Globals",
            ExportTarget::Workspace => "Workspace",
        }
    }

    fn supports_native(&self) -> bool {
        matches!(self, ExportTarget::Collection { .. })
    }

    fn supports_secrets(&self) -> bool {
        matches!(self, ExportTarget::Environment { .. } | ExportTarget::Globals)
    }

    fn default_format(&self) -> ExportFormat {
        if matches!(self, ExportTarget::Workspace) { ExportFormat::Zip } else { ExportFormat::Postman }
    }
}

/// The file shape to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportFormat {
    #[default]
    Postman,
    Native,
    Zip,
}

impl ExportFormat {
    fn label(self) -> &'static str {
        match self {
            ExportFormat::Postman => "Postman JSON",
            ExportFormat::Native => "Postguy native JSON",
            ExportFormat::Zip => "Workspace zip",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            ExportFormat::Postman | ExportFormat::Native => "json",
            ExportFormat::Zip => "zip",
        }
    }

    fn default_name(self, target: &ExportTarget) -> String {
        let base = target.name().replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "-");
        match (target, self) {
            (ExportTarget::Collection { .. }, ExportFormat::Postman) => format!("{base}.postman.json"),
            (ExportTarget::Collection { .. }, ExportFormat::Native) => format!("{base}.json"),
            (ExportTarget::Environment { .. }, _) => format!("{base}.postman_environment.json"),
            (ExportTarget::Globals, _) => "globals.postman_globals.json".to_string(),
            (ExportTarget::Workspace, _) => format!("{base}.zip"),
            _ => format!("{base}.json"),
        }
    }
}

/// Dialog state owned by the app.
#[derive(Debug, Clone, Default)]
pub struct ExportState {
    pub open: bool,
    pub target: Option<ExportTarget>,
    pub format: ExportFormat,
    pub include_secrets: bool,
    pub busy: bool,
    pub error: Option<String>,
    pub done: Option<String>,
}

/// The action the app performs (save picker, then serialise/write).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRequest {
    pub target: ExportTarget,
    pub format: ExportFormat,
    pub include_secrets: bool,
    pub default_name: String,
    pub extension: &'static str,
}

/// Open the dialog for a target, resetting per-run options.
pub fn open(state: &mut ExportState, target: ExportTarget) {
    state.open = true;
    state.format = target.default_format();
    state.include_secrets = false;
    state.busy = false;
    state.error = None;
    state.done = None;
    state.target = Some(target);
}

/// Draw the modal while `state.open` and a target are set.
pub fn export_dialog(ctx: &egui::Context, state: &mut ExportState) -> Option<ExportRequest> {
    let theme = Theme::current(ctx);
    let Some(target) = state.target.clone() else {
        state.open = false;
        return None;
    };
    if !matches!(state.format, ExportFormat::Zip) && matches!(target, ExportTarget::Workspace) {
        state.format = ExportFormat::Zip;
    }
    let mut request = None;
    let was_open = state.open;
    widgets::modal(ctx, "export-dialog", "Export", &mut state.open, |ui| {
        ui.set_min_width(420.0);
        ui.horizontal(|ui| {
            ui.label("Export");
            ui.label(RichText::new(target.label()).strong());
            ui.label(RichText::new(target.name()).color(theme.palette.text_muted));
        });
        ui.separator();
        if target.supports_native() {
            let mut chosen = format!("{:?}", state.format);
            for candidate in [ExportFormat::Postman, ExportFormat::Native] {
                if widgets::radio(ui, &mut chosen, &format!("{:?}", candidate), candidate.label()).changed() {
                    state.format = candidate;
                }
            }
        } else if matches!(target, ExportTarget::Workspace) {
            ui.label(RichText::new(ExportFormat::Zip.label()).strong());
            state.format = ExportFormat::Zip;
        } else {
            ui.label(RichText::new(ExportFormat::Postman.label()).strong());
            state.format = ExportFormat::Postman;
        }
        if target.supports_secrets() {
            ui.add_space(4.0);
            let mut secrets = state.include_secrets;
            if widgets::checkbox(ui, &mut secrets, "Include secret values").changed() {
                state.include_secrets = secrets;
            }
            ui.label(RichText::new("Secret values are omitted by default; the secret flag is kept.").small().color(theme.palette.text_muted));
        }
        ui.add_space(8.0);
        if state.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Exporting…");
            });
        }
        if let Some(error) = &state.error {
            ui.label(RichText::new(error).color(theme.palette.danger));
        }
        if let Some(done) = &state.done {
            ui.label(RichText::new(done).color(theme.palette.success));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled(!state.busy, egui::Button::new("Export…")).clicked() {
                request = Some(ExportRequest {
                    target: target.clone(),
                    format: state.format,
                    include_secrets: state.include_secrets,
                    default_name: state.format.default_name(&target),
                    extension: state.format.extension(),
                });
            }
        });
    });
    if !was_open || (!state.open && request.is_none()) {
        return None;
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_choose_sensible_defaults() {
        let collection = ExportTarget::Collection { id: "c".into(), name: "Payments/API".into() };
        assert_eq!(collection.default_format(), ExportFormat::Postman);
        assert_eq!(ExportFormat::Postman.default_name(&collection), "Payments-API.postman.json");
        assert_eq!(ExportFormat::Native.default_name(&collection), "Payments-API.json");
        let environment = ExportTarget::Environment { id: "e".into(), name: "Staging".into() };
        assert_eq!(ExportFormat::Postman.default_name(&environment), "Staging.postman_environment.json");
        let workspace = ExportTarget::Workspace;
        assert_eq!(workspace.default_format(), ExportFormat::Zip);
        assert_eq!(ExportFormat::Zip.default_name(&workspace), "Workspace.zip");
        assert!(collection.supports_native() && !collection.supports_secrets());
        assert!(environment.supports_secrets() && !environment.supports_native());
    }
}
