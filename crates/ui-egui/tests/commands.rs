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
