//! Export dialog: target-specific formats, the secret toggle and the request it emits.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_ui_egui::export::{ExportFormat, ExportRequest, ExportState, ExportTarget, export_dialog};

#[derive(Default)]
struct State {
    export: ExportState,
    request: Option<ExportRequest>,
}

fn make_harness(export: ExportState) -> Harness<'static, State> {
    Harness::new_ui_state(
        |ui, state: &mut State| {
            if let Some(request) = export_dialog(ui.ctx(), &mut state.export) {
                state.request = Some(request);
            }
        },
        State { export, request: None },
    )
}

#[test]
fn a_collection_can_export_native_json() {
    let mut state = ExportState::default();
    postguy_ui_egui::export::open(&mut state, ExportTarget::Collection { id: "c1".into(), name: "Payments".into() });
    let mut harness = make_harness(state);
    harness.run();
    assert_eq!(harness.state().export.format, ExportFormat::Postman);
    harness.get_by_label("Postguy native JSON").click();
    harness.run();
    assert_eq!(harness.state().export.format, ExportFormat::Native);
    harness.get_by_label("Export…").click();
    harness.run();
    let request = harness.state().request.clone().expect("request");
    assert_eq!(request.format, ExportFormat::Native);
    assert_eq!(request.default_name, "Payments.json");
    assert_eq!(request.extension, "json");
}

#[test]
fn environments_offer_secrets_and_workspaces_are_always_zips() {
    let mut state = ExportState::default();
    postguy_ui_egui::export::open(&mut state, ExportTarget::Environment { id: "e1".into(), name: "Staging".into() });
    let mut harness = make_harness(state);
    harness.run();
    harness.get_by_label("Include secret values").click();
    harness.run();
    assert!(harness.state().export.include_secrets);
    harness.get_by_label("Export…").click();
    harness.run();
    assert_eq!(harness.state().request.as_ref().map(|request| request.extension), Some("json"));
    assert!(harness.state().request.as_ref().is_some_and(|request| request.include_secrets));

    let mut state = ExportState::default();
    postguy_ui_egui::export::open(&mut state, ExportTarget::Workspace);
    let mut harness = make_harness(state);
    harness.run();
    assert_eq!(harness.state().export.format, ExportFormat::Zip);
    harness.get_by_label("Export…").click();
    harness.run();
    assert_eq!(harness.state().request.as_ref().map(|request| request.extension), Some("zip"));
}
