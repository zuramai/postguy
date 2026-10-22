//! Code snippets: the right-panel widget with a language menu, formatting
//! options, Copy and the generated text. Generation lives in `postguy-snippets` so the panel,
//! the automation tool and the future CLI share one implementation.

use egui::{RichText, Ui};
use postguy_model::HttpRequest;
use postguy_snippets::{Language, Options, generate};

use crate::theme::Theme;
use crate::widgets;

/// The language type re-exported for the panel API.
pub type SnippetLanguage = Language;

/// Per-shell panel state: the chosen language and formatting knobs.
#[derive(Debug, Clone)]
pub struct SnippetPanelState {
    pub language: SnippetLanguage,
    /// Spaces per level; 0 means one tab.
    pub indent: usize,
    pub include_boilerplate: bool,
}

impl Default for SnippetPanelState {
    fn default() -> Self {
        SnippetPanelState { language: Language::Curl, indent: 2, include_boilerplate: true }
    }
}

impl SnippetPanelState {
    fn options(&self) -> Options {
        Options { indent: self.indent, include_boilerplate: self.include_boilerplate }
    }
}

/// The right-panel widget: language menu, options, copy button and the syntax-coloured text.
pub fn snippet_panel(ui: &mut Ui, request: &HttpRequest, state: &mut SnippetPanelState) {
    let theme = Theme::current(ui.ctx());
    let text = generate(state.language, request, state.options());
    ui.label(RichText::new("Code snippet").font(theme.semibold_font(14.0)));
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::icon_button_with(ui, "copy", "Copy", "Copy snippet").clicked() {
                ui.ctx().copy_text(text.clone());
            }
            let mut chosen = Some(state.language.label().to_string());
            let groups = [widgets::SelectGroup {
                label: None,
                items: Language::all().into_iter().map(|language| widgets::SelectItem::new(language.label(), language.label())).collect(),
            }];
            if widgets::select(ui, "snippet-language", &mut chosen, &groups, state.language.label()) {
                state.language = Language::all().into_iter().find(|language| Some(language.label().to_string()) == chosen).unwrap_or(Language::Curl);
            }
            let options = widgets::icon_button(ui, "settings", "Snippet options");
            egui::Popup::menu(&options).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.set_min_width(160.0);
                ui.label(RichText::new("Indent").small().color(theme.palette.text_muted));
                for (label, value) in [("2 spaces", 2), ("4 spaces", 4), ("Tab", 0)] {
                    if ui.selectable_label(state.indent == value, label).clicked() {
                        state.indent = value;
                    }
                }
                ui.separator();
                let mut boilerplate = state.include_boilerplate;
                if widgets::checkbox(ui, &mut boilerplate, "Include boilerplate").changed() {
                    state.include_boilerplate = boilerplate;
                }
            });
        });
    });
    ui.separator();
    let mut text = generate(state.language, request, state.options());
    let editor = widgets::CodeEditor::new(code_language(state.language)).rows(20).read_only(true).id(egui::Id::new("snippet-editor"));
    let response = editor.show(ui, &mut text);
    response.on_hover_text("Variables are shown unresolved here; the Console shows the resolved URL.");
    ui.label(RichText::new(state.language.label()).small().color(theme.palette.text_muted));
}

/// Which token rules a generated snippet gets.
fn code_language(language: Language) -> widgets::CodeLanguage {
    match language {
        Language::Curl => widgets::CodeLanguage::Shell,
        Language::RawHttp => widgets::CodeLanguage::Http,
        Language::Fetch | Language::NodeAxios => widgets::CodeLanguage::JavaScript,
        Language::PythonRequests => widgets::CodeLanguage::Python,
        Language::Go => widgets::CodeLanguage::Go,
        Language::Rust => widgets::CodeLanguage::Rust,
        Language::PowerShell => widgets::CodeLanguage::PowerShell,
    }
}
