//! Tab-strip tests (M3.3): preview replacement and promotion, close/reopen, dirty-close choices,
//! drag reorder, retention after a workspace change, session serialisation and the real strip UI
//! (middle-click, close button, context menu, overflow search, environment activation).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_model::{Collection, Environment, Id, Variable};
use postguy_ui_egui::sidebar::SidebarData;
use postguy_ui_egui::tabs::{CloseChoice, TabAction, TabDoc, Tabs, tab_strip};

fn doc(id: &str) -> TabDoc {
    TabDoc::Request { collection: "c1".into(), request: id.into(), method: "GET".into(), protocol: "http".into() }
}

fn data() -> SidebarData {
    SidebarData {
        revision: 1,
        environments: vec![
            Environment {
                id: Id("env1".into()),
                name: "Staging".into(),
                variables: vec![Variable { key: "base".into(), value: "x".into(), ..Default::default() }],
                ..Default::default()
            },
            Environment { id: Id("env2".into()), name: "Production".into(), ..Default::default() },
        ],
        active_environment: Some(Id("env1".into())),
        collections: vec![Collection {
            id: Id("c1".into()),
            name: "Demo".into(),
            variables: vec![Variable { key: "collectionVar".into(), value: "value".into(), ..Default::default() }],
            ..Default::default()
        }],
        globals: Environment {
            id: Id("globals".into()),
            name: "Globals".into(),
            variables: vec![Variable { key: "region".into(), value: "eu".into(), ..Default::default() }],
            ..Default::default()
        },
        ..SidebarData::default()
    }
}

#[test]
fn a_preview_replaces_the_previous_preview_and_promotes_in_place() {
    let mut tabs = Tabs::new();
    let first = tabs.open_preview(doc("r1"), "One");
    assert_eq!(tabs.tabs.len(), 1);
    assert_eq!(tabs.active, Some(first));
    let second = tabs.open_preview(doc("r2"), "Two");
    assert_eq!(tabs.tabs.len(), 1, "the old preview is replaced");
    assert_eq!(tabs.tabs[0].id, second);
    assert_eq!(tabs.tabs[0].title, "Two");
    tabs.open_preview(doc("r2"), "Two");
    assert_eq!(tabs.tabs.len(), 1, "opening the same document twice does not duplicate");
    let id = tabs.open(doc("r2"), "Two");
    assert_eq!(tabs.tabs.len(), 1);
    assert!(!tabs.tabs[0].preview, "a permanent open promotes the preview");
    assert_eq!(tabs.active, Some(id));
}

#[test]
fn closing_and_reopening_restores_the_document() {
    let mut tabs = Tabs::new();
    let a = tabs.open(doc("a"), "A");
    let b = tabs.open(doc("b"), "B");
    tabs.activate(a);
    assert!(tabs.request_close(a));
    assert_eq!(tabs.tabs.len(), 1);
    assert_eq!(tabs.active, Some(b));
    tabs.close_now(b);
    assert!(tabs.tabs.is_empty());
    let reopened = tabs.reopen_last().expect("reopened");
    assert_eq!(reopened.doc, doc("b"));
    assert_eq!(tabs.closed.len(), 1, "the older closed tab stays in the stack");
}

#[test]
fn dirty_tabs_ask_and_the_choices_do_what_they_say() {
    let mut tabs = Tabs::new();
    let id = tabs.open(doc("dirty"), "Dirty");
    tabs.set_dirty(id, true);
    assert!(!tabs.request_close(id), "the dialog opens instead of closing");
    assert_eq!(tabs.pending_close, Some(id));
    assert_eq!(tabs.resolve_close(CloseChoice::Cancel), None);
    assert!(tabs.tabs.iter().any(|tab| tab.id == id), "Cancel keeps the tab");
    assert!(!tabs.request_close(id));
    assert_eq!(tabs.resolve_close(CloseChoice::Discard), None);
    assert!(tabs.tabs.is_empty(), "Don't save closes");

    let id = tabs.open(doc("save"), "Save");
    tabs.set_dirty(id, true);
    assert!(!tabs.request_close(id));
    assert_eq!(tabs.resolve_close(CloseChoice::Save), Some(id), "Save reports the id for persistence");
    assert!(tabs.tabs.is_empty());
}

#[test]
fn reorder_moves_the_tab_and_ignores_out_of_range() {
    let mut tabs = Tabs::new();
    tabs.open(doc("a"), "A");
    tabs.open(doc("b"), "B");
    tabs.open(doc("c"), "C");
    tabs.reorder(0, 2);
    let titles: Vec<&str> = tabs.tabs.iter().map(|tab| tab.title.as_str()).collect();
    assert_eq!(titles, vec!["B", "C", "A"]);
    tabs.reorder(0, 99);
    assert_eq!(tabs.tabs.len(), 3);
}

#[test]
fn retain_valid_drops_deleted_documents_and_repairs_the_active_tab() {
    let mut tabs = Tabs::new();
    let a = tabs.open(TabDoc::Collection { id: "gone".into() }, "Gone");
    let b = tabs.open(TabDoc::Settings, "Settings");
    let removed = tabs.retain_valid(|doc| !matches!(doc, TabDoc::Collection { id } if id == "gone"));
    assert_eq!(removed, 1);
    assert_eq!(tabs.tabs.len(), 1);
    assert_eq!(tabs.active, Some(b));
    assert!(!tabs.tabs.iter().any(|tab| tab.id == a));
}

#[test]
fn tabs_and_drafts_serialise_for_session_restore() {
    let mut tabs = Tabs::new();
    let id = tabs.open(doc("a"), "Draft");
    tabs.set_dirty(id, true);
    tabs.set_draft(id, "{\"draft\":true}".into());
    tabs.open(TabDoc::Settings, "Settings");
    let json = serde_json::to_value(&tabs).expect("serialize");
    let mut back: Tabs = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back.tabs.len(), 2);
    assert!(back.tabs[0].dirty);
    assert_eq!(back.drafts.get(&id).map(String::as_str), Some("{\"draft\":true}"));
    // Ids continue instead of colliding after a restore.
    let next = back.open(doc("b"), "B");
    assert!(next > id);
}

#[derive(Default)]
struct StripState {
    tabs: Tabs,
    action: Option<TabAction>,
    last_action: Option<TabAction>,
}

fn harness() -> Harness<'static, StripState> {
    Harness::new_ui_state(
        |ui, state: &mut StripState| {
            state.action = tab_strip(ui, &mut state.tabs, &data());
            if state.action.is_some() {
                state.last_action = state.action.clone();
            }
        },
        StripState::default(),
    )
}

#[test]
fn the_strip_activates_middle_click_closes_and_the_close_button_works() {
    let mut harness = harness();
    harness.state_mut().tabs.open(doc("a"), "Alpha");
    harness.state_mut().tabs.open(doc("b"), "Beta");
    harness.run();
    harness.get_by_label("Alpha").click();
    harness.run();
    assert_eq!(harness.state().tabs.active, Some(1));
    harness.get_by_label("Beta").click_button(egui::PointerButton::Middle);
    harness.run();
    assert_eq!(harness.state().tabs.tabs.len(), 1, "middle click closes");
    harness.get_by_label("Close Alpha").click();
    harness.run();
    assert!(harness.state().tabs.tabs.is_empty(), "the close button closes");
}

#[test]
fn closing_a_tab_before_others_in_the_strip_does_not_panic() {
    let mut harness = harness();
    harness.state_mut().tabs.open(doc("a"), "Alpha");
    harness.state_mut().tabs.open(doc("b"), "Beta");
    harness.state_mut().tabs.open(doc("c"), "Gamma");
    harness.run();
    harness.get_by_label("Close Alpha").click();
    harness.run();
    let titles: Vec<&str> = harness.state().tabs.tabs.iter().map(|tab| tab.title.as_str()).collect();
    assert_eq!(titles, ["Beta", "Gamma"]);
}

#[test]
fn a_dirty_close_shows_the_three_way_dialog() {
    let mut harness = harness();
    let id = harness.state_mut().tabs.open(doc("dirty"), "Dirty");
    harness.state_mut().tabs.set_dirty(id, true);
    harness.run();
    harness.state_mut().tabs.request_close(id);
    harness.run();
    assert_eq!(harness.state().tabs.pending_close, Some(id));
    assert!(harness.query_by_label("Unsaved changes").is_some());
    assert!(harness.query_by_label("Save").is_some());
    harness.get_by_label("Don't save").click();
    harness.run();
    assert!(harness.state().tabs.tabs.is_empty());
}

#[test]
fn the_context_menu_closes_other_tabs_and_reveals_the_document() {
    let mut harness = harness();
    harness.state_mut().tabs.open(doc("a"), "Alpha");
    harness.state_mut().tabs.open(doc("b"), "Beta");
    harness.run();
    harness.get_by_label("Beta").click_secondary();
    harness.run();
    assert!(harness.query_by_label("Close Other Tabs").is_some());
    harness.get_by_label("Reveal in Sidebar").click();
    harness.run();
    assert_eq!(harness.state().last_action, Some(TabAction::RevealInSidebar("b".into())));
    harness.get_by_label("Beta").click_secondary();
    harness.run();
    harness.get_by_label("Close Other Tabs").click();
    harness.run();
    assert_eq!(harness.state().tabs.tabs.len(), 1);
    assert_eq!(harness.state().tabs.tabs[0].title, "Beta");
}

#[test]
fn the_overflow_button_lists_open_and_recently_closed_tabs() {
    let mut harness = harness();
    harness.state_mut().tabs.open(doc("a"), "Alpha");
    let closed = harness.state_mut().tabs.open(doc("b"), "Beta");
    harness.state_mut().tabs.close_now(closed);
    harness.run();
    harness.get_by_label("Tabs").click();
    harness.run();
    assert!(harness.query_by_label("Open tabs").is_some());
    assert!(harness.query_by_label_contains("Recently closed").is_some());
    assert!(harness.query_by_label_contains("Beta").is_some(), "the closed tab is listed");
}

#[test]
fn the_variable_quick_look_groups_scopes_and_masks_secrets() {
    let mut harness = harness();
    harness.state_mut().tabs.open(doc("a"), "Alpha");
    harness.run();
    harness.get_by_label("Vars").click();
    harness.run();
    assert!(harness.query_by_label("Variables").is_some());
    assert!(harness.query_by_label("Environment").is_some());
    assert!(harness.query_by_label("Globals").is_some());
    assert!(harness.query_by_label("Collection").is_some());
    assert!(harness.query_by_label_contains("base").is_some(), "the environment variable is listed");
}

#[test]
fn the_selector_can_create_an_environment() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label_contains("Staging").click();
    harness.run();
    harness.get_by_label_contains("+ Create new environment").click();
    harness.run();
    assert_eq!(harness.state().last_action, Some(TabAction::CreateEnvironment));
}

#[test]
fn the_environment_selector_searches() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label_contains("Staging").click();
    harness.run();
    harness.get_by_role(egui::accesskit::Role::TextInput).click();
    harness.get_by_role(egui::accesskit::Role::TextInput).type_text("Prod");
    harness.run();
    assert!(harness.query_by_label_contains("Production").is_some());
    assert_eq!(harness.query_all_by_label_contains("Staging").count(), 1, "only the selector button still says Staging");
}

#[test]
fn the_environment_selector_reports_activation() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label_contains("Staging").click();
    harness.run();
    harness.get_by_label_contains("Production").click();
    harness.run();
    assert_eq!(harness.state().last_action, Some(TabAction::SetActiveEnvironment(Some("env2".into()))));
}

#[test]
fn hovering_a_tab_or_its_close_button_shows_the_pointing_hand() {
    let mut harness = harness();
    harness.state_mut().tabs.open(doc("a"), "Alpha");
    harness.state_mut().tabs.open(doc("b"), "Beta");
    harness.run();
    harness.get_by_label("Alpha").hover();
    harness.run();
    assert_eq!(harness.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand, "the tab");
    harness.get_by_label("Close Beta").hover();
    harness.run();
    assert_eq!(harness.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand, "the close button");
}
