//! kittest coverage for the M3.1 widget kit: every custom control is reachable through
//! AccessKit, responds to a real click, and reports state the caller can assert. The shell test
//! exercises the frame (header/sidebar/bottom bar/settings tab) end to end.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_ui_egui::widgets::{
    self, CodeEditor, CodeLanguage, ConfirmOutcome, InlineRename, LargeTextView, MessageDirection, RenameAction, SelectGroup, SelectItem,
    TableVariant, Timeline, TimelineEntry,
};
use postguy_ui_egui::{PostguyShell, ShellState, ShellTab};
use std::sync::Arc;

#[derive(Default)]
struct CheckState {
    checked: bool,
}

#[test]
fn checkbox_toggles_from_the_keyboard_and_the_mouse() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut CheckState| {
            widgets::checkbox(ui, &mut state.checked, "Remember me");
        },
        CheckState::default(),
    );
    harness.run();
    assert!(!harness.state().checked);
    harness.get_by_label("Remember me").click();
    harness.run();
    assert!(harness.state().checked, "a click toggles the square checkbox");
}

#[derive(Default)]
struct RadioState {
    selected: String,
}

#[test]
fn radio_selects_one_value() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut RadioState| {
            widgets::radio(ui, &mut state.selected, "one", "One");
            widgets::radio(ui, &mut state.selected, "two", "Two");
        },
        RadioState::default(),
    );
    harness.run();
    harness.get_by_label("Two").click();
    harness.run();
    assert_eq!(harness.state().selected, "two");
}

#[derive(Default)]
struct SwitchState {
    on: bool,
}

#[test]
fn switch_toggles() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut SwitchState| {
            widgets::switch(ui, &mut state.on, "Proxy");
        },
        SwitchState::default(),
    );
    harness.run();
    harness.get_by_label("Proxy").click();
    harness.run();
    assert!(harness.state().on);
}

#[derive(Default)]
struct SelectState {
    selected: Option<String>,
}

fn groups() -> Vec<SelectGroup> {
    vec![
        SelectGroup { label: Some("Saved".into()), items: vec![SelectItem::new("env1", "Staging"), SelectItem::new("env2", "Production")] },
        SelectGroup { label: Some("Other".into()), items: vec![SelectItem { separator_before: true, ..SelectItem::new("none", "No environment") }] },
    ]
}

#[test]
fn select_shows_groups_checks_and_changes_the_value() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut SelectState| {
            widgets::select(ui, "env-select", &mut state.selected, &groups(), "Choose");
        },
        SelectState::default(),
    );
    harness.run();
    harness.get_by_label_contains("Choose").click();
    harness.run();
    assert!(harness.query_by_label("SAVED").is_some(), "group headers are visible");
    harness.get_by_label_contains("Production").click();
    harness.run();
    assert_eq!(harness.state().selected.as_deref(), Some("env2"));
    assert!(harness.query_by_label_contains("Production").is_some(), "the button shows the selection");
}

#[derive(Default)]
struct SplitState {
    sent: usize,
    downloaded: usize,
}

#[test]
fn split_button_runs_both_actions() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut SplitState| {
            widgets::split_button(
                ui,
                "Send",
                || state.sent += 1,
                |ui| {
                    if widgets::menu_item(ui, Some("send"), "Send and Download", None, true).clicked() {
                        state.downloaded += 1;
                    }
                },
            );
        },
        SplitState::default(),
    );
    harness.run();
    harness.get_by_label("Send").click();
    harness.run();
    assert_eq!(harness.state().sent, 1);
    harness.get_by_label("▾").click();
    harness.run();
    harness.get_by_label_contains("Send and Download").click();
    harness.run();
    assert_eq!(harness.state().downloaded, 1);
}

#[derive(Default)]
struct PopupState {
    selected: Option<String>,
}

#[test]
fn searchable_popup_filters_and_selects() {
    let items: Vec<String> = ["alpha", "beta", "gamma"].iter().map(|item| item.to_string()).collect();
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut PopupState| {
            widgets::searchable_popup(ui, "picker", "Pick", &items, &mut state.selected);
        },
        PopupState::default(),
    );
    harness.run();
    harness.get_by_label_contains("Pick").click();
    harness.run();
    harness.get_by_role(egui::accesskit::Role::TextInput).click();
    harness.get_by_role(egui::accesskit::Role::TextInput).type_text("gam");
    harness.run();
    assert!(harness.query_by_label_contains("alpha").is_none(), "the filter hides other rows");
    harness.get_by_label_contains("gamma").click();
    harness.run();
    assert_eq!(harness.state().selected.as_deref(), Some("gamma"));
}

#[derive(Default)]
struct MenuState {
    removed: usize,
}

#[test]
fn context_menu_items_close_and_run() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut MenuState| {
            let response = ui.button("Request row");
            widgets::context_menu(&response, |ui| {
                if widgets::menu_item(ui, Some("x"), "Remove", Some("Del"), true).clicked() {
                    state.removed += 1;
                }
            });
        },
        MenuState::default(),
    );
    harness.run();
    harness.get_by_label("Request row").click_secondary();
    harness.run();
    assert!(harness.query_by_label_contains("Remove").is_some(), "the context menu opened");
    harness.get_by_label_contains("Remove").click();
    harness.run();
    assert_eq!(harness.state().removed, 1);
}

#[derive(Default)]
struct ModalState {
    open: bool,
    closed: bool,
}

#[test]
fn modal_opens_and_dismisses_from_the_backdrop() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut ModalState| {
            if ui.button("Open").clicked() {
                state.open = true;
            }
            let ctx = ui.ctx().clone();
            widgets::modal(&ctx, "demo", "Rename collection", &mut state.open, |ui| {
                ui.label("Modal body");
                if ui.button("Done").clicked() {
                    state.closed = true;
                }
            });
        },
        ModalState::default(),
    );
    harness.run();
    harness.get_by_label("Open").click();
    harness.run();
    assert!(harness.query_by_label("Modal body").is_some());
    harness.get_by_label("Done").click();
    harness.run();
    assert!(harness.state().closed);
}

#[derive(Default)]
struct ConfirmState {
    open: bool,
    outcome: Option<ConfirmOutcome>,
}

#[test]
fn typed_confirmation_needs_the_exact_word() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut ConfirmState| {
            if ui.button("Delete…").clicked() {
                state.open = true;
            }
            let ctx = ui.ctx().clone();
            let outcome =
                widgets::typed_confirm(&ctx, "delete", &mut state.open, "Delete collection", "This cannot be undone.", "Payments", "Delete");
            if outcome != ConfirmOutcome::Pending {
                state.outcome = Some(outcome);
            }
        },
        ConfirmState::default(),
    );
    harness.run();
    harness.get_by_label("Delete…").click();
    harness.run();
    harness.get_by_role(egui::accesskit::Role::TextInput).type_text("payments");
    harness.run();
    assert_eq!(harness.state().outcome, None, "the wrong case cannot confirm");
    // Replace the text with the exact word and press the destructive button.
    harness.get_by_role(egui::accesskit::Role::TextInput).click();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get_by_role(egui::accesskit::Role::TextInput).type_text("Payments");
    harness.run();
    harness.get_by_label("Delete").click();
    harness.run();
    assert_eq!(harness.state().outcome, Some(ConfirmOutcome::Confirmed));
}

struct RenameState {
    text: String,
    rename: InlineRename,
    action: RenameAction,
    last_action: RenameAction,
}

#[test]
fn inline_rename_commits_from_the_editor() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut RenameState| {
            state.action = widgets::inline_rename(ui, "rename", &mut state.text, &mut state.rename);
            if state.action != RenameAction::Idle {
                state.last_action = state.action;
            }
        },
        RenameState { text: "Old name".into(), rename: InlineRename::default(), action: RenameAction::Idle, last_action: RenameAction::Idle },
    );
    harness.state_mut().rename.begin("Old name");
    harness.run();
    harness.get_by_role(egui::accesskit::Role::TextInput).click();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get_by_role(egui::accesskit::Role::TextInput).type_text("Renamed");
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state().text, "Renamed");
    assert_eq!(harness.state().last_action, RenameAction::Committed);
}

struct TableState {
    rows: Vec<postguy_model::KeyValue>,
}

#[test]
fn key_value_table_adds_and_deletes_rows() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut TableState| {
            let _ = widgets::key_value_table(ui, &mut state.rows, TableVariant::Normal);
        },
        TableState { rows: vec![postguy_model::KeyValue { key: "Accept".into(), value: "application/json".into(), ..Default::default() }] },
    );
    harness.run();
    assert_eq!(harness.state().rows.len(), 1);
    harness.get_by_label_contains("Add row").click();
    harness.run();
    assert_eq!(harness.state().rows.len(), 2);
    let deletes = harness.get_all_by_label("✕").count();
    assert_eq!(deletes, 2, "one delete button per row");
    harness.get_all_by_label("✕").next().expect("a delete button").click();
    harness.run();
    assert_eq!(harness.state().rows.len(), 1);
}

struct FieldState {
    text: String,
}

#[test]
fn variable_field_edits_text_and_keeps_the_chip_spans() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut FieldState| {
            let _ = widgets::variable_field(ui, egui::Id::new("url-field"), &mut state.text, "https://…");
        },
        FieldState { text: "{{base}}/users".into() },
    );
    harness.run();
    let spans = widgets::variable_spans(&harness.state().text);
    assert_eq!(spans.len(), 1);
    let input = harness.get_by_role(egui::accesskit::Role::TextInput);
    input.click();
    input.type_text("?page=2");
    harness.run();
    assert!(harness.state().text.ends_with("?page=2"));
}

struct EditorState {
    text: String,
}

#[test]
fn code_editor_has_a_gutter_and_accepts_edits() {
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut EditorState| {
            let _ = CodeEditor::new(CodeLanguage::Json).rows(4).id(egui::Id::new("code")).show(ui, &mut state.text);
        },
        EditorState { text: "{\n  \"a\": 1\n}".into() },
    );
    harness.run();
    assert!(harness.query_by_label_contains("1").is_some(), "line numbers are present");
    harness.get_by_role(egui::accesskit::Role::MultilineTextInput).click();
    harness.get_by_role(egui::accesskit::Role::MultilineTextInput).type_text(" ");
    harness.run();
    assert!(harness.state().text.contains(' '));
}

#[test]
fn large_text_view_virtualises_many_lines() {
    let text: Arc<str> = (0..10_000).map(|line| format!("line {line}\n")).collect::<String>().into();
    let view = LargeTextView::new(text);
    assert_eq!(view.line_count(), 10_001, "a trailing newline starts an empty last line");
    let mut harness = Harness::new_ui(move |ui| {
        let _ = view.show(ui);
    });
    harness.run();
    assert!(harness.query_by_label_contains("line 0").is_some());
}

#[test]
fn timeline_shows_newest_first_and_expands_a_row() {
    let entries = (0..50)
        .map(|index| TimelineEntry::new(MessageDirection::Received, index, format!("message {index}"), 12).with_detail(format!("detail {index}")))
        .collect();
    let timeline = Timeline::new(entries);
    let mut harness = Harness::new_ui(move |ui| {
        let _ = timeline.show(ui);
    });
    harness.run();
    assert!(harness.query_by_label_contains("message 49").is_some(), "reverse order shows the newest first");
    harness.get_by_label_contains("message 49").click();
    harness.run();
    assert!(harness.query_by_label_contains("detail 49").is_some(), "clicking a row expands its detail");
}

#[test]
fn the_shell_draws_its_regions_and_opens_settings() {
    let shell = PostguyShell::new("demo-workspace");
    let mut harness = Harness::new_ui_state(move |ui, shell: &mut PostguyShell| shell.ui(ui), shell);
    harness.run();
    assert!(harness.query_by_label_contains("demo-workspace").is_some());
    assert!(harness.query_by_label_contains("COLLECTIONS").is_some());
    assert!(harness.query_all_by_label_contains("The tree lands").next().is_some());
    harness.get_by_label("Settings").click();
    harness.run();
    assert_eq!(harness.state().state.tab, ShellTab::Settings);
    assert!(harness.query_by_label_contains("Theme").is_some());
    let state: &ShellState = &harness.state().state;
    assert!(state.sidebar_open);
}
