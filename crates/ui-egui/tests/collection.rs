//! Collection/folder detail tests (M3.7): overview editing, auth, variables, scripts, folder
//! detail, counts and the header menu intents.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_model::auth::BasicAuth;
use postguy_model::{AuthConfig, Collection, Folder, Id, Item, Request, RequestKind, Variable};
use postguy_ui_egui::collection::{CollectionAction, CollectionDetail, DetailState, DetailTab};

struct State {
    collection: Collection,
    folder: Option<Id>,
    detail: DetailState,
    action: Option<CollectionAction>,
    last: Option<CollectionAction>,
}

fn request(id: &str) -> Item {
    Item::Request(Box::new(Request { id: Id(id.into()), name: id.into(), kind: RequestKind::Http(Default::default()), ..Default::default() }))
}

fn sample() -> Collection {
    let folder = Folder {
        id: Id("folder1".into()),
        name: "v1".into(),
        variables: vec![Variable { key: "inner".into(), value: "1".into(), ..Default::default() }],
        items: vec![request("r1")],
        ..Default::default()
    };
    Collection {
        id: Id("col1".into()),
        name: "Payments".into(),
        description: "The payments API.".into(),
        variables: vec![Variable { key: "baseUrl".into(), value: "https://api.test".into(), ..Default::default() }],
        items: vec![Item::Folder(Box::new(folder)), request("r2")],
        ..Default::default()
    }
}

fn harness(folder: bool) -> Harness<'static, State> {
    let collection = sample();
    Harness::new_ui_state(
        |ui, state: &mut State| {
            let mut detail = CollectionDetail { collection: &mut state.collection, folder: state.folder.clone(), state: &mut state.detail };
            state.action = detail.show(ui);
            if state.action.is_some() {
                state.last = state.action.clone();
            }
        },
        State {
            collection,
            folder: if folder { Some(Id("folder1".into())) } else { None },
            detail: DetailState::default(),
            action: None,
            last: None,
        },
    )
}

#[test]
fn overview_counts_and_description_edit_autosave() {
    let mut harness = harness(false);
    harness.run();
    assert!(harness.query_by_label_contains("1 folder(s)").is_some());
    assert!(harness.query_by_label_contains("2 request(s)").is_some());
    let editor = harness.get_all_by_role(egui::accesskit::Role::MultilineTextInput).next().expect("description editor");
    editor.click();
    editor.type_text(" More.");
    harness.run();
    assert!(harness.state().collection.description.contains("More."));
    assert_eq!(harness.state().last, Some(CollectionAction::Save));
}

#[test]
fn auth_variables_and_scripts_tabs_edit_and_save() {
    let mut first = harness(false);
    first.state_mut().detail.tab = DetailTab::Authorization;
    first.run();
    first.get_by_label_contains("No Auth").click();
    first.run();
    first.get_by_label_contains("Basic Auth").click();
    first.run();
    assert!(matches!(first.state().collection.auth, Some(AuthConfig::Basic(_))));
    assert_eq!(first.state().last, Some(CollectionAction::Save));

    let mut second = harness(false);
    second.state_mut().detail.tab = DetailTab::Variables;
    second.run();
    assert!(second.query_all_by_value("baseUrl").next().is_some());
    let deletes = second.get_all_by_label("Delete variable").count();
    assert_eq!(deletes, 1);
    second.get_all_by_label("Delete variable").next().expect("delete").click();
    second.run();
    assert!(second.state().collection.variables.is_empty());
    assert_eq!(second.state().last, Some(CollectionAction::Save));

    let mut third = harness(false);
    third.state_mut().detail.tab = DetailTab::Scripts;
    third.run();
    let editor = third.get_all_by_role(egui::accesskit::Role::MultilineTextInput).next().expect("script editor");
    editor.click();
    editor.type_text("pm.test('ok')");
    third.run();
    assert!(third.state().collection.scripts.prerequest.as_ref().is_some_and(|script| script.text.contains("pm.test")));
}

#[test]
fn folder_detail_has_no_runs_tab_and_edits_the_folder() {
    let mut harness = harness(true);
    harness.run();
    assert!(harness.query_by_label("Runs").is_none(), "folders have no Runs tab");
    harness.get_by_label("Detail name").click();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get_by_label("Detail name").type_text("v2");
    harness.run();
    let name = match harness.state().collection.items.first() {
        Some(Item::Folder(folder)) => folder.name.clone(),
        _ => String::new(),
    };
    assert_eq!(name, "v2");
    assert_eq!(harness.state().last, Some(CollectionAction::Save));
    assert_eq!(harness.state().detail.tab, DetailTab::Overview);
}

#[test]
fn the_header_menu_emits_duplicate_export_and_delete() {
    let mut harness = harness(false);
    harness.run();
    harness.get_by_label("...").click();
    harness.run();
    harness.get_by_label("Duplicate").click();
    harness.run();
    assert_eq!(harness.state().last, Some(CollectionAction::Duplicate));
    harness.get_by_label("...").click();
    harness.run();
    harness.get_by_label("Export").click();
    harness.run();
    assert_eq!(harness.state().last, Some(CollectionAction::Export));
    harness.get_by_label("...").click();
    harness.run();
    harness.get_by_label("Delete").click();
    harness.run();
    assert_eq!(harness.state().last, Some(CollectionAction::Delete));
}

#[test]
fn collection_auth_does_not_offer_inherit() {
    let mut harness = harness(false);
    harness.state_mut().collection.auth = Some(AuthConfig::Basic(Box::new(BasicAuth { username: "u".into(), ..Default::default() })));
    harness.state_mut().detail.tab = DetailTab::Authorization;
    harness.run();
    assert!(harness.query_by_label_contains("Inherit").is_none(), "collection-level auth has no Inherit");
    assert!(harness.query_by_label_contains("Basic Auth").is_some());
}
