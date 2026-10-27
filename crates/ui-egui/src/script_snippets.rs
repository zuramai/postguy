//! Script editors with a Snippets menu, shared by the request builder and the collection and
//! folder detail pages. The snippets are written for this project.

use egui::{Align, Layout, RichText, Ui};
use postguy_model::Script;

use crate::theme::Theme;
use crate::widgets::{self, CodeEditor, CodeLanguage, menu_item};

/// `(menu label, code)` pairs for the pre-request editor.
pub const PRE_REQUEST: &[(&str, &str)] = &[
    ("Set a variable", "pg.variables.set(\"name\", \"value\");"),
    ("Send a request", "pg.sendRequest(\"https://example.com\", (err, res) => {\n  console.log(res.code);\n});"),
];

/// `(menu label, code)` pairs for the post-response editor.
pub const POST_RESPONSE: &[(&str, &str)] = &[
    ("Status code is 200", "pg.test(\"Status code is 200\", () => {\n  pg.response.to.have.status(200);\n});"),
    (
        "Response time is below 500 ms",
        "pg.test(\"Response time is below 500 ms\", () => {\n  pg.expect(pg.response.responseTime).to.be.below(500);\n});",
    ),
    ("JSON body has a property", "pg.test(\"Body has an id\", () => {\n  pg.expect(pg.response.json()).to.have.property(\"id\");\n});"),
    ("Save a value to the environment", "pg.environment.set(\"token\", pg.response.json().token);"),
];

/// `(menu label, code)` pairs for the request's Tests script.
pub const TESTS: &[(&str, &str)] = &[
    ("Status code is 200", "pg.test(\"Status code is 200\", () => pg.expect(pg.response.code).to.equal(200));"),
    ("Response time below 1s", "pg.test(\"Response time below 1 s\", () => pg.expect(pg.response.responseTime).to.be.below(1000));"),
    ("JSON field equals", "pg.test(\"status is ok\", () => pg.expect(pg.response.json().status).to.equal(\"ok\"));"),
    ("Add report column", "pg.report.set(\"subject_id\", pg.response.json().subject_id);"),
    ("Save runtime variable", "pg.setVar(\"session_id\", pg.response.json().session_id);"),
];

/// `(menu label, code)` pairs for the gRPC Pre-invoke editor.
pub const GRPC_PRE_INVOKE: &[(&str, &str)] = &[
    ("Set a variable", "pg.variables.set(\"name\", \"value\");"),
    ("Add metadata", "pg.request.headers.add({ key: \"x-trace-id\", value: \"123\" });"),
];

/// `(menu label, code)` pairs for the gRPC On message editor.
pub const GRPC_ON_MESSAGE: &[(&str, &str)] =
    &[("Message has a field", "pg.test(\"message has text\", () => pg.expect(pg.response.json()).to.have.property(\"text\"));")];

/// `(menu label, code)` pairs for the gRPC Post-response editor.
pub const GRPC_POST_RESPONSE: &[(&str, &str)] = &[
    ("Status is OK", "pg.test(\"Status is OK\", () => pg.expect(pg.response.code).to.equal(0));"),
    ("Response time below 1s", "pg.test(\"Response time below 1 s\", () => pg.expect(pg.response.responseTime).to.be.below(1000));"),
    ("Save a value", "pg.environment.set(\"id\", pg.response.json().id);"),
];

/// `(menu label, code)` pairs for the gRPC Test script.
pub const GRPC_TESTS: &[(&str, &str)] = GRPC_POST_RESPONSE;

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
