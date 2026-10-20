//! Code snippets (PROMPT.md 5.8, M4.2): the right-panel widget with a language menu, formatting
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

/// The right-panel widget: language menu, options, copy button and generated text.
pub fn snippet_panel(ui: &mut Ui, request: &HttpRequest, state: &mut SnippetPanelState) {
    let theme = Theme::current(ui.ctx());
    ui.heading("Code snippet");
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let text = generate(state.language, request, state.options());
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(text);
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
    let response = ui.add(egui::TextEdit::multiline(&mut text).code_editor().desired_width(f32::INFINITY).desired_rows(24).interactive(false));
    response.on_hover_text("Variables are shown unresolved here; the Console shows the resolved URL.");
    ui.label(RichText::new(state.language.label()).small().color(theme.palette.text_muted));
}
