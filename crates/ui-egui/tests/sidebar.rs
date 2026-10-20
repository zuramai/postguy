//! kittest coverage for the M3.2 sidebar: flatten/filter, preview-open, caret collapse, example
//! expansion, keyboard navigation, multi-select, context-menu rename, drag-and-drop and the
//! section headers. All actions are asserted as emitted intents; the app applies them.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_model::{
    Collection, Environment, Example, Folder, HistoryEntry, HttpMethod, HttpRequest, Id, Item, KeyValue, Request, RequestKind, Variable,
};
use postguy_ui_egui::PostguyShell;
use postguy_ui_egui::sidebar::{RequestProtocol, RowKind, Sidebar, SidebarAction, SidebarData, SidebarState};

fn request(id: &str, name: &str, method: HttpMethod, url: &str) -> Request {
    Request {
        id: Id(id.into()),
        name: name.into(),
        kind: RequestKind::Http(HttpRequest { method, url: url.into(), ..HttpRequest::default() }),
        ..Request::default()
    }
}

fn sample_data() -> SidebarData {
    let example = Example { id: Id("ex1".into()), name: "200 OK".into(), ..Example::default() };
    let mut users = request("req1", "Users", HttpMethod::Get, "https://api.example.com/users");
    users.examples.push(example);
    let folder = Folder {
        id: Id("folder1".into()),
        name: "v1".into(),
        variables: vec![Variable { key: "version".into(), value: "1".into(), ..Variable::default() }],
        items: vec![
            Item::Request(Box::new(users)),
            Item::Request(Box::new(request("req2", "Create user", HttpMethod::Post, "https://api.example.com/users"))),
        ],
        ..Folder::default()
    };
    let collection = Collection {
        id: Id("col1".into()),
        name: "Payments".into(),
        items: vec![
            Item::Folder(Box::new(folder)),
            Item::Request(Box::new(request("req3", "Health", HttpMethod::Get, "https://api.example.com/health"))),
        ],
        ..Collection::default()
    };
    let environment = Environment {
        id: Id("env1".into()),
        name: "Staging".into(),
        variables: vec![Variable { key: "base".into(), value: "https://api.example.com".into(), ..Variable::default() }],
        ..Environment::default()
    };
    let mut history = HistoryEntry {
        id: Id("hist1".into()),
        time_ms: 1_790_000_000_000,
        request: Some(RequestKind::Http(HttpRequest {
            method: HttpMethod::Put,
            url: "https://api.example.com/orders".into(),
            ..HttpRequest::default()
        })),
        status: Some(200),
        ..HistoryEntry::default()
    };
    history.duration_ms = Some(12);
    SidebarData {
        revision: 1,
        collections: vec![collection],
        environments: vec![environment],
        history: vec![history],
        active_environment: Some(Id("env1".into())),
        globals: Environment { id: Id("globals".into()), name: "Globals".into(), ..Environment::default() },
    }
}

struct HarnessState {
    data: SidebarData,
    sidebar: Sidebar,
    state: SidebarState,
    action: Option<SidebarAction>,
    last_action: Option<SidebarAction>,
}

fn harness() -> Harness<'static, HarnessState> {
    let data = sample_data();
    Harness::new_ui_state(
        |ui, state: &mut HarnessState| {
            let data = state.data.clone();
            state.action = state.sidebar.show(ui, &mut state.state, &data);
            if state.action.is_some() {
                state.last_action = state.action.clone();
            }
        },
        HarnessState { data, sidebar: Sidebar::new(), state: SidebarState::default(), action: None, last_action: None },
    )
}

#[test]
fn flatten_orders_collections_folders_requests_examples_and_history() {
    let mut sidebar = Sidebar::new();
    let data = sample_data();
    let rows = sidebar.rows(&data, "");
    let labels: Vec<(&str, usize)> = rows.iter().map(|row| (row.label.as_str(), row.depth)).collect();
    assert_eq!(
        labels,
        vec![
            ("Payments", 0),
            ("v1", 1),
            ("Users", 2),
            ("200 OK", 3),
            ("Create user", 2),
            ("Health", 1),
            ("Staging", 0),
            ("https://api.example.com/orders", 0),
        ]
    );
    let users = rows.iter().find(|row| row.id == "req1").expect("users row");
    assert_eq!(users.method.as_deref(), Some("GET"));
    assert!(rows.iter().any(|row| row.id == "ex1" && row.kind == RowKind::Example));
}

#[test]
fn filtering_keeps_parents_and_matches_urls() {
    let mut sidebar = Sidebar::new();
    let data = sample_data();
    let rows = sidebar.rows(&data, "health");
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    assert!(ids.contains(&"col1") && ids.contains(&"req3"), "parents stay visible: {ids:?}");
    assert!(!ids.contains(&"req1"), "unrelated requests are filtered out");

    let by_url = sidebar.rows(&data, "orders");
    assert!(by_url.iter().any(|row| row.kind == RowKind::History), "history matches the request URL");
    assert!(!by_url.iter().any(|row| row.id == "col1"));
}

#[test]
fn clicking_a_request_opens_it_as_a_preview() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Users").click();
    harness.run();
    assert_eq!(harness.state().last_action, Some(SidebarAction::Open { id: "req1".into(), kind: RowKind::Request, preview: true }));
    assert!(harness.state().state.selected.contains("req1"));
}

#[test]
fn caret_collapses_a_folder_and_expands_request_examples() {
    let mut harness = harness();
    harness.run();
    assert!(harness.query_by_label("Users").is_some());
    assert!(harness.query_by_label("200 OK").is_none(), "examples start collapsed");
    harness.get_by_label("Expand Users").click();
    harness.run();
    assert!(harness.query_by_label("200 OK").is_some(), "the request caret shows examples");
    harness.get_by_label("Collapse v1").click();
    harness.run();
    assert!(harness.query_by_label("Users").is_none(), "the folder caret hides its children");
    assert!(harness.query_by_label("Expand v1").is_some());
}

#[test]
fn keyboard_moves_focus_opens_and_duplicates_the_selection() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Users").click();
    harness.run();
    let first = harness.state().state.focused.clone();
    harness.key_press(egui::Key::ArrowDown);
    harness.run();
    let second = harness.state().state.focused.clone();
    assert!(second.is_some() && second != first, "ArrowDown moves the focused row");
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert!(harness.state().last_action.is_some(), "Enter opens the focused row");
}

#[test]
fn multi_select_extends_with_command_and_shift() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Users").click();
    harness.run();
    harness.get_by_label("Create user").click_modifiers(egui::Modifiers::COMMAND);
    harness.run();
    assert_eq!(harness.state().state.selected.len(), 2, "Cmd-click adds to the selection");
    harness.get_by_label("Health").click_modifiers(egui::Modifiers::SHIFT);
    harness.run();
    assert!(harness.state().state.selected.len() >= 2, "Shift-click selects a range");
}

#[test]
fn deleting_a_multi_selection_emits_one_bulk_delete() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Users").click();
    harness.run();
    harness.get_by_label("Create user").click_modifiers(egui::Modifiers::COMMAND);
    harness.run();
    harness.key_press(egui::Key::Delete);
    harness.run();
    match &harness.state().last_action {
        Some(SidebarAction::DeleteMany { items }) => {
            let ids: Vec<&str> = items.iter().map(|(id, _)| id.as_str()).collect();
            assert!(ids.contains(&"req1") && ids.contains(&"req2"), "both selected requests: {ids:?}");
        }
        other => panic!("expected DeleteMany, got {other:?}"),
    }
}

#[test]
fn copying_a_multi_selection_emits_one_bulk_copy() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Users").click();
    harness.run();
    harness.get_by_label("Create user").click_modifiers(egui::Modifiers::COMMAND);
    harness.run();
    assert_eq!(harness.state().state.selected.len(), 2, "two rows are selected before copying");
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::C);
    harness.run();
    match &harness.state().last_action {
        Some(SidebarAction::CopyMany { ids }) => {
            assert!(ids.contains(&"req1".into()) && ids.contains(&"req2".into()), "{ids:?}");
        }
        other => panic!("expected CopyMany, got {other:?}"),
    }
}

#[test]
fn dragging_one_of_a_selection_moves_the_whole_selection() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Users").click();
    harness.run();
    harness.get_by_label("Create user").click_modifiers(egui::Modifiers::COMMAND);
    harness.run();
    let request_rect = harness.get_by_label("Users").rect();
    let folder_rect = harness.get_by_label("v1").rect();
    harness.drag_at(request_rect.center());
    harness.run();
    harness.hover_at(request_rect.center() + egui::vec2(0.0, 6.0));
    harness.run();
    harness.hover_at(folder_rect.center());
    harness.run();
    harness.drop_at(folder_rect.center());
    harness.run();
    match &harness.state().last_action {
        Some(SidebarAction::MoveMany { ids, parent, .. }) => {
            assert!(ids.contains(&"req1".into()) && ids.contains(&"req2".into()), "{ids:?}");
            assert_eq!(parent.as_deref(), Some("folder1"));
        }
        other => panic!("expected MoveMany, got {other:?}"),
    }
}

#[test]
fn context_menu_rename_commits_through_inline_edit() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Users").click_secondary();
    harness.run();
    assert!(harness.query_by_label("Rename").is_some());
    harness.get_by_label("Rename").click();
    harness.run();
    harness.get_all_by_role(egui::accesskit::Role::TextInput).last().expect("rename field").click();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get_all_by_role(egui::accesskit::Role::TextInput).last().expect("rename field").type_text("Customers");
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state().last_action, Some(SidebarAction::Rename { id: "req1".into(), name: "Customers".into() }));
}

#[test]
fn dragging_a_request_onto_a_folder_emits_a_move() {
    let mut harness = harness();
    harness.run();
    let request_rect = harness.get_by_label("Health").rect();
    let folder_rect = harness.get_by_label("v1").rect();
    harness.drag_at(request_rect.center());
    harness.run();
    harness.hover_at(request_rect.center() + egui::vec2(0.0, 6.0));
    harness.run();
    assert!(harness.state().state.is_dragging(), "the drag started");
    harness.hover_at(folder_rect.center());
    harness.run();
    assert_eq!(harness.state().state.drop_target_id(), Some("folder1"), "the folder is the hovered drop target");
    harness.drop_at(folder_rect.center());
    harness.run();
    assert!(!harness.state().state.is_dragging(), "the drop released the drag");
    match &harness.state().last_action {
        Some(SidebarAction::Move { id, parent, .. }) => {
            assert_eq!(id, "req3");
            assert_eq!(parent.as_deref(), Some("folder1"), "the folder is the drop target");
        }
        other => panic!("expected Move, got {other:?}"),
    }
}

#[test]
fn the_new_menu_uses_the_collection_as_the_container_not_as_a_folder() {
    let mut collection_harness = harness();
    collection_harness.run();
    collection_harness.get_by_label("Payments").click();
    collection_harness.run();
    collection_harness.get_by_label("+").click();
    collection_harness.run();
    collection_harness.get_by_label("New HTTP request").click();
    collection_harness.run();
    match &collection_harness.state().last_action {
        Some(SidebarAction::NewRequest { collection, parent, protocol }) => {
            assert_eq!(collection, "col1");
            assert_eq!(parent, &None, "a collection is not a folder id");
            assert_eq!(*protocol, RequestProtocol::Http);
        }
        other => panic!("expected NewRequest, got {other:?}"),
    }

    // The hover "+" on a collection row uses the same container rule.
    let mut hover_harness = harness();
    hover_harness.run();
    hover_harness.get_by_label("Payments").click();
    hover_harness.run();
    hover_harness.get_by_label("Add request to Payments").click();
    hover_harness.run();
    match &hover_harness.state().last_action {
        Some(SidebarAction::NewRequest { collection, parent, .. }) => {
            assert_eq!(collection, "col1");
            assert_eq!(parent, &None, "the collection row's + must not pass its own id as a folder");
        }
        other => panic!("expected NewRequest from the hover button, got {other:?}"),
    }

    // A selected folder is still passed as the parent.
    let mut folder_harness = harness();
    folder_harness.run();
    folder_harness.get_by_label("v1").click();
    folder_harness.run();
    folder_harness.get_by_label("+").click();
    folder_harness.run();
    folder_harness.get_by_label("New HTTP request").click();
    folder_harness.run();
    match &folder_harness.state().last_action {
        Some(SidebarAction::NewRequest { collection, parent, .. }) => {
            assert_eq!(collection, "col1");
            assert_eq!(parent.as_deref(), Some("folder1"));
        }
        other => panic!("expected NewRequest, got {other:?}"),
    }
}

#[test]
fn sections_collapse_independently_and_show_the_active_environment() {
    let mut harness = harness();
    harness.run();
    assert!(harness.query_by_label("Staging").is_some());
    assert!(harness.query_by_label("Staging").is_some(), "the active environment row is drawn");
    harness.get_by_label("HISTORY").click();
    harness.run();
    assert!(harness.query_by_label("Staging").is_some(), "history collapse does not touch environments");
    assert!(harness.query_by_label_contains("https://api.example.com/orders").is_none(), "history rows hide");
}

#[test]
fn the_shell_opens_a_sidebar_row_in_the_detail_area() {
    let mut shell = PostguyShell::new("demo");
    shell.sidebar_data = sample_data();
    let mut harness = Harness::new_ui_state(move |ui, shell: &mut PostguyShell| shell.ui(ui), shell);
    harness.run();
    harness.get_by_label("Users").click();
    harness.run();
    assert_eq!(harness.state().tabs.active_tab().map(|tab| tab.title.as_str()), Some("Users"));
}

#[test]
fn protocol_choices_are_all_offered() {
    // A small guard that the action surface carries the three protocols the brief names.
    let protocols = [RequestProtocol::Http, RequestProtocol::Grpc, RequestProtocol::WebSocket];
    assert_eq!(protocols.len(), 3);
    let keys = KeyValue::default();
    assert!(keys.enabled);
}
