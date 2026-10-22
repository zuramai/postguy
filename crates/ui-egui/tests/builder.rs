//! Request-builder UI tests: send/save intents, the tab set, auth switching, body mode
//! switching, settings toggles and cURL paste detection.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_model::{AuthConfig, Body, HttpRequest, RawLanguage};
use postguy_ui_egui::builder::{BodyDrafts, BuilderAction, BuilderTab, RequestBuilder};
use postguy_ui_egui::widgets::field::VariableInfo;

struct State {
    name: String,
    description: String,
    draft: HttpRequest,
    bodies: BodyDrafts,
    dirty: bool,
    tab: BuilderTab,
    action: Option<BuilderAction>,
    last: Option<BuilderAction>,
    variables: Vec<VariableInfo>,
    undo_paste: bool,
    inherited: Option<String>,
}

fn harness() -> Harness<'static, State> {
    Harness::new_ui_state(
        |ui, state: &mut State| {
            let mut oauth2_action = None;
            let mut builder = RequestBuilder {
                name: &mut state.name,
                description: &mut state.description,
                collection_name: "Payments",
                draft: &mut state.draft,
                bodies: &mut state.bodies,
                dirty: &mut state.dirty,
                tab: &mut state.tab,
                variables: &state.variables,
                sending: None,
                undo_paste: state.undo_paste,
                inherited_auth: state.inherited.as_deref(),
                oauth2_action: &mut oauth2_action,
                graphql_schema: None,
            };
            let action = builder.show(ui);
            state.action = if let Some(action) = oauth2_action { Some(BuilderAction::OAuth2(action)) } else { action };
            if state.action.is_some() {
                state.last = state.action.clone();
            }
        },
        State {
            name: "Echo".into(),
            description: String::new(),
            draft: HttpRequest { url: "https://api.example.com/users".into(), ..HttpRequest::default() },
            bodies: BodyDrafts::default(),
            dirty: false,
            tab: BuilderTab::Params,
            action: None,
            last: None,
            variables: vec![
                VariableInfo { name: "base".into(), value: "https://api.example.com".into(), secret: false, scope: "environment".into() },
                VariableInfo { name: "token".into(), value: "hunter2".into(), secret: true, scope: "environment".into() },
            ],
            undo_paste: false,
            inherited: None,
        },
    )
}

#[test]
fn send_and_save_emit_their_intents() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Send").click();
    harness.run();
    assert_eq!(harness.state().last, Some(BuilderAction::Send { download: false }));

    harness.get_by_label("Body").click();
    harness.run();
    harness.state_mut().dirty = true;
    harness.run();
    harness.get_by_label("Save").click();
    harness.run();
    assert_eq!(harness.state().last, Some(BuilderAction::Save));
}

#[test]
fn authorization_switches_to_basic_and_shows_fields() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Authorization").click();
    harness.run();
    harness.get_by_label_contains("Inherit").click();
    harness.run();
    harness.get_by_label_contains("Basic Auth").click();
    harness.run();
    assert!(matches!(harness.state().draft.auth, AuthConfig::Basic(_)));
    assert!(harness.query_by_label("Username").is_some());
    assert!(harness.query_by_label("Password").is_some());
}

#[test]
fn advanced_auth_types_switch_and_show_their_fields() {
    let mut jwt_harness = harness();
    jwt_harness.run();
    jwt_harness.get_by_label("Authorization").click();
    jwt_harness.run();
    jwt_harness.get_by_label_contains("Inherit").click();
    jwt_harness.run();
    jwt_harness.get_by_label_contains("JWT Bearer").click();
    jwt_harness.run();
    assert!(matches!(jwt_harness.state().draft.auth, AuthConfig::Jwt(_)));
    assert!(jwt_harness.query_by_label("Payload (JSON claims)").is_some());
    assert!(jwt_harness.query_by_label("Algorithm").is_some());
    let mut aws_harness = harness();
    aws_harness.run();
    aws_harness.get_by_label("Authorization").click();
    aws_harness.run();
    aws_harness.get_by_label_contains("Inherit").click();
    aws_harness.run();
    aws_harness.get_by_label_contains("AWS Signature v4").click();
    aws_harness.run();
    assert!(matches!(aws_harness.state().draft.auth, AuthConfig::AwsV4(_)));
    assert!(aws_harness.query_by_label("Access key").is_some());
    assert!(aws_harness.query_by_label("Region").is_some());

    let mut oauth_harness = harness();
    oauth_harness.run();
    oauth_harness.get_by_label("Authorization").click();
    oauth_harness.run();
    oauth_harness.get_by_label_contains("Inherit").click();
    oauth_harness.run();
    oauth_harness.get_by_label_contains("OAuth 2.0").click();
    oauth_harness.run();
    assert!(matches!(oauth_harness.state().draft.auth, AuthConfig::OAuth2(_)));
    assert!(oauth_harness.query_by_label("Get New Access Token").is_some());
    oauth_harness.get_by_label("Get New Access Token").click_accesskit();
    oauth_harness.run();
    assert_eq!(oauth_harness.state().last, Some(BuilderAction::OAuth2(postguy_ui_egui::auth::OAuth2Action::Grant)));

    let mut gql_harness = harness();
    gql_harness.state_mut().draft.body =
        Body::GraphQL { query: "query { __typename }".into(), variables: "{}".into(), operation_name: None, extras: Default::default() };
    gql_harness.run();
    gql_harness.get_by_label("Body").click();
    gql_harness.run();
    gql_harness.get_by_label("Refresh schema").click();
    gql_harness.run();
    assert_eq!(gql_harness.state().last, Some(BuilderAction::IntrospectGraphql));
}

#[test]
fn body_mode_switches_and_keeps_the_raw_text_available() {
    let mut harness = harness();
    harness.state_mut().draft.body = Body::Raw { language: RawLanguage::Json, text: "{\"a\":1}".into(), extras: Default::default() };
    harness.run();
    harness.get_by_label("Body").click();
    harness.run();
    harness.get_by_label_contains("Raw").click();
    harness.run();
    harness.get_by_label_contains("None").click();
    harness.run();
    assert!(matches!(harness.state().draft.body, Body::None));
    assert_eq!(harness.state().bodies.raw, "{\"a\":1}", "the raw draft is kept across modes");
}

#[test]
fn settings_toggles_persist_into_the_request_settings() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Settings").click();
    harness.run();
    assert!(harness.state().draft.settings.verify_ssl);
    harness.get_by_label("Disable cookie jar").click();
    harness.run();
    assert!(harness.state().draft.settings.cookie_jar_disabled);
    assert!(harness.state().dirty);
}

#[test]
fn pasting_a_curl_command_emits_the_parse_intent() {
    let mut harness = harness();
    harness.run();
    let url = harness.get_all_by_role(egui::accesskit::Role::TextInput).nth(1).expect("url field");
    url.click();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get_all_by_role(egui::accesskit::Role::TextInput).nth(1).expect("url field").type_text("curl https://example.com -H 'X: y'");
    harness.run();
    match &harness.state().last {
        Some(BuilderAction::ParseCurl(text)) => assert!(text.starts_with("curl ")),
        other => panic!("expected ParseCurl, got {other:?}"),
    }
}

#[test]
fn enter_in_the_url_sends() {
    let mut harness = harness();
    harness.run();
    harness.get_all_by_role(egui::accesskit::Role::TextInput).nth(1).expect("url").click();
    harness.key_press(egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state().last, Some(BuilderAction::Send { download: false }));
}

#[test]
fn autocomplete_inserts_a_variable() {
    let mut harness = harness();
    harness.run();
    harness.get_all_by_role(egui::accesskit::Role::TextInput).nth(1).expect("url").click();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get_all_by_role(egui::accesskit::Role::TextInput).nth(1).expect("url").type_text("/{{ba");
    harness.run();
    harness.get_by_label("{{base}}").click();
    harness.run();
    assert!(harness.state().draft.url.contains("{{base}}"), "autocomplete inserted the variable: {}", harness.state().draft.url);
}

#[test]
fn the_curl_undo_hint_emits_the_undo_action() {
    let mut harness = harness();
    harness.state_mut().undo_paste = true;
    harness.run();
    assert!(harness.query_by_label("cURL imported.").is_some());
    harness.get_by_label("Undo").click();
    harness.run();
    assert_eq!(harness.state().last, Some(BuilderAction::UndoPaste));
}

#[test]
fn the_docs_tab_edits_the_description_and_marks_dirty() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Docs").click();
    harness.run();
    let input = harness.get_all_by_role(egui::accesskit::Role::MultilineTextInput).last().expect("docs editor");
    input.click();
    input.type_text("How the endpoint behaves");
    harness.run();
    assert!(harness.state().description.contains("endpoint"));
    assert!(harness.state().dirty);
}

#[test]
fn the_send_button_and_the_request_tabs_show_the_pointing_hand() {
    let mut harness = harness();
    harness.run();
    for label in ["Send", "Send options", "Params", "Authorization", "Body"] {
        harness.get_by_label(label).hover();
        harness.run();
        assert_eq!(harness.output().platform_output.cursor_icon, egui::CursorIcon::PointingHand, "{label}");
    }
}
