//! Script editors with a Snippets menu, shared by the request builder and the collection and
//! folder detail pages. The snippets are written for this project.

use egui::{Align, Layout, RichText, Ui};
use postguy_model::Script;

use crate::theme::Theme;
use crate::widgets::{self, CodeEditor, CodeLanguage, menu_item};

/// `(menu label, code)` pairs for the pre-request editor.
pub const PRE_REQUEST: &[(&str, &str)] = &[
    ("Set a variable", "pm.variables.set(\"name\", \"value\");"),
    ("Send a request", "pm.sendRequest(\"https://example.com\", (err, res) => {\n  console.log(res.code);\n});"),
];

/// `(menu label, code)` pairs for the post-response editor.
pub const POST_RESPONSE: &[(&str, &str)] = &[
    ("Status code is 200", "pm.test(\"Status code is 200\", () => {\n  pm.response.to.have.status(200);\n});"),
    (
        "Response time is below 500 ms",
        "pm.test(\"Response time is below 500 ms\", () => {\n  pm.expect(pm.response.responseTime).to.be.below(500);\n});",
    ),
    ("JSON body has a property", "pm.test(\"Body has an id\", () => {\n  pm.expect(pm.response.json()).to.have.property(\"id\");\n});"),
    ("Save a value to the environment", "pm.environment.set(\"token\", pm.response.json().token);"),
];

/// One script editor under `label` with a Snippets menu. Returns true when the text changed.
pub fn script_editor(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    id: egui::Id,
    rows: usize,
    script: &mut Option<Script>,
    snippets: &[(&str, &str)],
) -> bool {
    let mut text = script.as_ref().map(|script| script.text.clone()).unwrap_or_default();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).small().color(theme.palette.text_muted));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let button = widgets::text_button(ui, widgets::ButtonKind::Ghost, None, "Snippets", true);
            egui::Popup::menu(&button).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
                ui.set_min_width(240.0);
                for (name, code) in snippets {
                    if menu_item(ui, None, name, None, true).clicked() {
                        if !text.trim().is_empty() {
                            text.push_str("\n\n");
                        }
                        text.push_str(code);
                        changed = true;
                    }
                }
            });
        });
    });
    if CodeEditor::new(CodeLanguage::JavaScript).rows(rows).id(id).show(ui, &mut text).changed() {
        changed = true;
    }
    if changed {
        let target = script.get_or_insert_with(Script::default);
        target.text = text;
    }
    changed
}
