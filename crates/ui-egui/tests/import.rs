//! Import dialog coverage: text submission, the Replace checkbox, and report rows that open
//! the imported document. Parsing/workspace writes are engine tests; this file tests the UI.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_interop::ImportReport;
use postguy_ui_egui::import::{ImportAction, ImportItemView, ImportState, ImportSummary, import_dialog};

#[derive(Default)]
struct State {
    import: ImportState,
    action: Option<ImportAction>,
}

fn harness(state: ImportState) -> Harness<'static, State> {
    Harness::new_ui_state(
        |ui, state: &mut State| {
            if let Some(action) = import_dialog(ui.ctx(), &mut state.import) {
                state.action = Some(action);
            }
        },
        State { import: state, action: None },
    )
}

#[test]
fn paste_import_returns_the_text_and_replace_preference() {
    let state = ImportState { open: true, text: "curl https://example.test".into(), ..ImportState::default() };
    let mut harness = harness(state);
    harness.run();
    harness.get_by_label("Replace existing").click();
    harness.run();
    assert!(harness.state().import.replace);
    harness.get_by_label("Import text").click();
    harness.run();
    let action = harness.state().action.clone().expect("action");
    assert_eq!(action, ImportAction::ImportText { text: "curl https://example.test".into(), replace: true });
}

#[test]
fn report_rows_name_warnings_and_open_the_imported_document() {
    let item = ImportItemView {
        source: "pasted".into(),
        kind: "collection".into(),
        name: "Payments".into(),
        id: Some("c1".into()),
        report: ImportReport { collections: 1, requests: 3, warnings: vec!["x: repaired".into()], ..ImportReport::default() },
        error: None,
    };
    let state = ImportState {
        open: true,
        summary: Some(ImportSummary { items: vec![item], problems: vec!["one file was skipped".into()] }),
        ..ImportState::default()
    };
    let mut harness = harness(state);
    harness.run();
    assert!(harness.query_by_label_contains("Imported 1 collection(s)").is_some());
    assert!(harness.query_by_label_contains("warning: x: repaired").is_some());
    assert!(harness.query_by_label_contains("one file was skipped").is_some());
    harness.get_by_label("Open").click();
    harness.run();
    assert_eq!(harness.state().action, Some(ImportAction::Open { index: 0 }));
}

#[test]
fn the_dialog_names_the_supported_formats_and_toggles_the_openapi_folder_layout() {
    let mut harness = harness(ImportState { open: true, ..ImportState::default() });
    harness.run();
    assert!(harness.query_by_label_contains("OpenAPI 3 / Swagger 2 (JSON or YAML), HAR, .proto files").is_some());
    assert!(!harness.state().import.folders_by_path);
    harness.get_by_label("OpenAPI: folders by path (default: by tag)").click();
    harness.run();
    assert!(harness.state().import.folders_by_path);
}
