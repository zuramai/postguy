//! Trash view tests: rows, relative deletion times and the Restore intent.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_ui_egui::trash::{TrashAction, TrashEntryView, trash_view};

#[test]
fn entries_render_and_restore_emits_the_id() {
    let entries = vec![
        TrashEntryView { id: "t1".into(), kind: "collection".into(), original: "collections/c1.json".into(), deleted_at_ms: 0, size_bytes: Some(42) },
        TrashEntryView { id: "t2".into(), kind: "environment".into(), original: "environments/e1.json".into(), deleted_at_ms: 0, size_bytes: None },
    ];
    let mut harness = Harness::new_ui_state(
        |ui, action: &mut Option<TrashAction>| {
            if let Some(next) = trash_view(ui, &entries) {
                *action = Some(next);
            }
        },
        None,
    );
    harness.run();
    assert!(harness.query_by_label_contains("collections/c1.json").is_some());
    assert!(harness.query_all_by_label_contains("environments/e1.json").next().is_some());
    assert_eq!(harness.get_all_by_label("Restore").count(), 2);
    harness.get_all_by_label("Restore").next().expect("restore").click();
    harness.run();
    assert_eq!(*harness.state(), Some(TrashAction::Restore("t1".into())));
}

#[test]
fn an_empty_trash_explains_itself() {
    let entries: Vec<TrashEntryView> = Vec::new();
    let mut harness = Harness::new_ui(move |ui| {
        let _ = trash_view(ui, &entries);
    });
    harness.run();
    assert!(harness.query_by_label_contains("The trash is empty").is_some());
}
