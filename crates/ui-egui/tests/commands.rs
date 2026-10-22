//! Command palette tests: filtering, command-only mode, keyboard selection and document items.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_ui_egui::commands::{CommandAction, CommandRegistry, PaletteItem, PaletteState, command_palette};
use postguy_ui_egui::tabs::TabDoc;

struct State {
    palette: PaletteState,
    registry: CommandRegistry,
    action: Option<CommandAction>,
    last: Option<CommandAction>,
}

fn harness() -> Harness<'static, State> {
    Harness::new_ui_state(
        |ui, state: &mut State| {
            let items = vec![PaletteItem {
                label: "List payments".into(),
                hint: "GET request".into(),
                doc: TabDoc::Request { collection: "c1".into(), request: "r1".into(), method: "GET".into(), protocol: "http".into() },
            }];
            state.action = command_palette(ui, &mut state.palette, &state.registry, &items);
            if state.action.is_some() {
                state.last = state.action.clone();
            }
        },
        State { palette: PaletteState { open: true, ..Default::default() }, registry: CommandRegistry::default(), action: None, last: None },
    )
}

#[test]
fn typing_filters_and_enter_runs_a_command() {
    let mut harness = harness();
    harness.run();
    let search = harness.get_by_role(egui::accesskit::Role::TextInput);
    search.click();
    search.type_text("globals");
    harness.run();
    assert!(harness.query_by_label("Open globals").is_some());
    assert!(harness.query_by_label("Open settings").is_none(), "non-matching commands are filtered");
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state().last, Some(CommandAction::OpenGlobals));
}

#[test]
fn the_gt_prefix_hides_workspace_items() {
    let mut harness = harness();
    harness.run();
    let search = harness.get_by_role(egui::accesskit::Role::TextInput);
    search.click();
    search.type_text("> payments");
    harness.run();
    assert!(harness.query_by_label("List payments").is_none(), "`>` restricts to commands");
    assert!(harness.query_by_label_contains("No matches").is_some());
}

#[test]
fn clicking_a_workspace_item_opens_its_document() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("List payments").click();
    harness.run();
    match &harness.state().last {
        Some(CommandAction::OpenDoc(TabDoc::Request { request, .. })) => assert_eq!(request, "r1"),
        other => panic!("expected OpenDoc, got {other:?}"),
    }
    assert!(!harness.state().palette.open, "choosing closes the palette");
}

struct ShellRun {
    shell: postguy_ui_egui::PostguyShell,
    /// The last command the shell emitted (the shell clears its own field every frame).
    last: Option<CommandAction>,
}

fn shell_harness() -> Harness<'static, ShellRun> {
    Harness::new_ui_state(
        |ui, run: &mut ShellRun| {
            run.shell.ui(ui);
            if run.shell.command_action.is_some() {
                run.last = run.shell.command_action.clone();
            }
        },
        ShellRun { shell: postguy_ui_egui::PostguyShell::new("demo"), last: None },
    )
}

#[test]
fn the_in_app_menu_runs_registry_commands() {
    use postguy_ui_egui::commands::{Placement, WindowCommand};
    let mut harness = shell_harness();
    harness.run();
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label("New ⏵").click();
    harness.run();
    harness.get_by_label("New collection").click();
    harness.run();
    assert_eq!(harness.state().last, Some(CommandAction::NewCollection));

    let mut harness = shell_harness();
    harness.run();
    harness.get_by_label("Window").click();
    harness.run();
    harness.get_by_label("Move & Resize ⏵").click();
    harness.run();
    harness.get_by_label("Halves ⏵").click();
    harness.run();
    harness.get_by_label("Left half").click();
    harness.run();
    assert_eq!(harness.state().last, Some(CommandAction::Window(WindowCommand::Place(Placement::LeftHalf))));
}

#[test]
fn zoom_setting_drives_the_context_zoom() {
    let mut harness = shell_harness();
    harness.state_mut().shell.settings_value.ui_zoom = 1.5;
    harness.run();
    assert!((harness.ctx.zoom_factor() - 1.5).abs() < 1e-3);
    harness.state_mut().shell.settings_value.ui_zoom = f32::NAN;
    harness.run();
    assert!((harness.ctx.zoom_factor() - 1.0).abs() < 1e-3, "a corrupt zoom falls back to 100 %");
}
