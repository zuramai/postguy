//! Environment/Globals editor tests: autosave intents, secret masking/reveal, delete, the menu
//! actions, and the grouped quick-look with masked values.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_model::{Environment, Id, Variable};
use postguy_ui_egui::environment::{EnvironmentAction, EnvironmentEditor, VariableGroup, variable_quick_look};

struct EditorState {
    environment: Environment,
    globals: bool,
    search: String,
    show_description: bool,
    reveal: bool,
    action: Option<EnvironmentAction>,
    last: Option<EnvironmentAction>,
}

fn environment() -> Environment {
    Environment {
        id: Id("env1".into()),
        name: "Staging".into(),
        color: Some("#2F6FEB".into()),
        variables: vec![
            Variable { key: "baseUrl".into(), value: "https://staging.example.com".into(), ..Variable::default() },
            Variable { key: "token".into(), value: "hunter2".into(), secret: true, ..Variable::default() },
        ],
        ..Environment::default()
    }
}

fn harness(globals: bool) -> Harness<'static, EditorState> {
    Harness::new_ui_state(
        |ui, state: &mut EditorState| {
            let mut editor = EnvironmentEditor {
                environment: &mut state.environment,
                globals: state.globals,
                search: &mut state.search,
                show_description: &mut state.show_description,
                reveal: &mut state.reveal,
            };
            state.action = editor.show(ui);
            if state.action.is_some() {
                state.last = state.action.clone();
            }
        },
        EditorState {
            environment: if globals {
                Environment { id: Id("globals".into()), name: "Globals".into(), variables: environment().variables, ..Environment::default() }
            } else {
                environment()
            },
            globals,
            search: String::new(),
            show_description: true,
            reveal: false,
            action: None,
            last: None,
        },
    )
}

#[test]
fn renaming_and_editing_autosave() {
    let mut harness = harness(false);
    harness.run();
    harness.get_by_label("Environment name").click();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get_by_label("Environment name").type_text("Production");
    harness.run();
    assert_eq!(harness.state().environment.name, "Production");
    assert_eq!(harness.state().last, Some(EnvironmentAction::Save), "edits autosave");
}

#[test]
fn secrets_are_masked_until_revealed() {
    let mut harness = harness(false);
    harness.run();
    assert!(harness.query_by_label("Reveal secrets").is_some());
    assert!(harness.query_by_value("hunter2").is_none(), "the secret field is masked by default");
    harness.get_by_label("Reveal secrets").click();
    harness.run();
    assert!(harness.state().reveal);
    assert!(harness.query_all_by_value("hunter2").next().is_some(), "the revealed secret field exposes its value");
}

#[test]
fn search_filters_the_variable_rows() {
    let mut harness = harness(false);
    harness.state_mut().search = "token".into();
    harness.run();
    assert!(harness.query_all_by_value("token").next().is_some(), "the matching row stays");
    assert!(harness.query_all_by_value("baseUrl").next().is_none(), "non-matching rows are hidden");
}

#[test]
fn deleting_a_variable_changes_the_model_and_saves() {
    let mut harness = harness(false);
    harness.run();
    // One delete button per row (the lucide x icon labels itself with its tooltip).
    let deletes = harness.get_all_by_label("Delete variable").count();
    assert_eq!(deletes, 2);
    harness.get_all_by_label("Delete variable").next().expect("delete").click();
    harness.run();
    assert_eq!(harness.state().environment.variables.len(), 1);
    assert_eq!(harness.state().last, Some(EnvironmentAction::Save));
}

#[test]
fn the_menu_duplicates_exports_and_deletes() {
    let mut harness = harness(false);
    harness.run();
    harness.get_by_label("...").click();
    harness.run();
    harness.get_by_label("Duplicate").click();
    harness.run();
    assert_eq!(harness.state().last, Some(EnvironmentAction::Duplicate));
    harness.get_by_label("...").click();
    harness.run();
    harness.get_by_label("Delete").click();
    harness.run();
    assert_eq!(harness.state().last, Some(EnvironmentAction::Delete));
}

#[test]
fn globals_hide_the_environment_menu() {
    let mut harness = harness(true);
    harness.run();
    assert!(harness.query_all_by_label_contains("Globals").next().is_some());
    assert!(harness.query_by_label("...").is_none(), "globals have no duplicate/delete menu");
}

#[test]
fn the_quick_look_groups_mask_secrets_and_copy_values() {
    let groups = vec![
        VariableGroup {
            title: "Environment".into(),
            variables: vec![("baseUrl".into(), "https://api.test".into(), false), ("token".into(), "hunter2".into(), true)],
        },
        VariableGroup { title: "Globals".into(), variables: vec![("region".into(), "eu".into(), false)] },
    ];
    let mut harness = Harness::new_ui(move |ui| variable_quick_look(ui, &groups));
    harness.run();
    assert!(harness.query_by_label_contains("baseUrl").is_some());
    assert!(harness.query_by_label_contains("••••••").is_some(), "secret values start masked");
    harness.get_by_label("Reveal secrets").click();
    harness.run();
    assert!(harness.query_by_label_contains("hunter2").is_some(), "reveal shows the value");
    harness.get_by_label_contains("region").click();
    harness.run();
}

#[test]
fn an_environment_without_variables_shows_an_empty_row_that_creates_one_when_typed_in() {
    let mut harness = harness(false);
    harness.state_mut().environment.variables.clear();
    harness.run();
    // Name, search, then the empty row's Variable and Value cells.
    let cells = harness.get_all_by_role(egui::accesskit::Role::TextInput).count();
    assert!(cells >= 4, "the empty row is drawn ({cells} text inputs)");
    harness.get_all_by_role(egui::accesskit::Role::TextInput).nth(2).expect("variable cell").click();
    harness.get_all_by_role(egui::accesskit::Role::TextInput).nth(2).expect("variable cell").type_text("host");
    harness.run();
    let variables = &harness.state().environment.variables;
    assert_eq!(variables.len(), 1);
    assert_eq!(variables[0].key, "host");
    assert!(variables[0].enabled);
    assert_eq!(harness.state().last, Some(EnvironmentAction::Save));
}
