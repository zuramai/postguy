//! gRPC builder UI tests: the Service definition tables, the method picker, the Test tab and
//! the Invoke intent.

use std::collections::BTreeMap;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_model::{GrpcFieldInfo, GrpcMessageInfo, GrpcMethodInfo, GrpcRequest, GrpcSchema, GrpcServiceInfo};
use postguy_ui_egui::{GrpcAction, GrpcBuilder, GrpcSchemaState, GrpcTab};

struct State {
    name: String,
    draft: GrpcRequest,
    dirty: bool,
    tab: GrpcTab,
    schema: GrpcSchemaState,
    last: Option<GrpcAction>,
}

fn field(name: &str, number: u32, label: &str) -> GrpcFieldInfo {
    GrpcFieldInfo { name: name.into(), json_name: name.into(), number, type_label: label.into(), type_full: label.into(), ..GrpcFieldInfo::default() }
}

fn echo_schema() -> GrpcSchema {
    let method = |name: &str, client: bool, server: bool| GrpcMethodInfo {
        name: name.into(),
        path: format!("echo.Echo/{name}"),
        input: "echo.EchoMessage".into(),
        output: "echo.EchoMessage".into(),
        client_streaming: client,
        server_streaming: server,
    };
    let mut messages = BTreeMap::new();
    messages.insert(
        "echo.EchoMessage".to_string(),
        GrpcMessageInfo { name: "echo.EchoMessage".into(), fields: vec![field("text", 1, "string"), field("number", 2, "int32")] },
    );
    GrpcSchema {
        files: vec!["echo/echo.proto".into()],
        services: vec![GrpcServiceInfo {
            name: "echo.Echo".into(),
            methods: vec![method("Say", false, false), method("Collect", true, false), method("Chat", true, true)],
        }],
        messages,
    }
}

fn harness(tab: GrpcTab, schema: GrpcSchemaState, method_path: &str) -> Harness<'static, State> {
    Harness::new_ui_state(
        |ui, state: &mut State| {
            let mut builder = GrpcBuilder {
                name: &mut state.name,
                collection_name: "Payments",
                draft: &mut state.draft,
                dirty: &mut state.dirty,
                tab: &mut state.tab,
                schema: &state.schema,
                variables: &[],
                sending: None,
                inherited_auth: None,
                id: "req-1",
            };
            if let Some(action) = builder.show(ui) {
                state.last = Some(action);
            }
        },
        State {
            name: "Say".into(),
            draft: GrpcRequest { url: "localhost:50051".into(), method_path: method_path.into(), ..GrpcRequest::default() },
            dirty: false,
            tab,
            schema,
            last: None,
        },
    )
}

#[test]
fn service_definition_renders_request_and_response_tables() {
    let mut harness = harness(GrpcTab::ServiceDefinition, GrpcSchemaState::Loaded(echo_schema()), "echo.Echo/Say");
    harness.run();
    assert!(harness.query_by_label("request").is_some());
    assert!(harness.query_by_label("response").is_some());
    assert!(harness.query_all_by_label("EchoMessage").count() >= 2, "both tables name their type");
    assert_eq!(harness.query_all_by_label("text").count(), 2, "`text` is listed under request and response");
    assert_eq!(harness.query_all_by_label("string").count(), 2, "`string` is listed under request and response");
    assert_eq!(harness.query_all_by_label("number").count(), 2, "`number` is listed under request and response");
    assert!(harness.query_by_label("Unary").is_some());
}

#[test]
fn method_picker_without_definition_points_to_service_definition_tab() {
    let mut harness = harness(GrpcTab::Message, GrpcSchemaState::NotLoaded, "");
    harness.run();
    harness.get_by_label("Select a method").click();
    harness.run();
    assert!(harness.query_by_label("Set the service definition first.").is_some());
    harness.get_by_label("Open Service definition").click();
    harness.run();
    assert_eq!(harness.state().tab, GrpcTab::ServiceDefinition);
}

#[test]
fn test_tab_matches_http_test_tab() {
    let mut harness = harness(GrpcTab::Test, GrpcSchemaState::NotLoaded, "");
    harness.run();
    assert!(harness.query_by_label("Assertions").is_some());
    assert!(harness.query_by_label("Test script").is_some());
}

#[test]
fn invoke_button_emits_invoke() {
    let mut harness = harness(GrpcTab::Message, GrpcSchemaState::NotLoaded, "echo.Echo/Say");
    harness.run();
    harness.get_by_label("Invoke").click();
    harness.run();
    assert_eq!(harness.state().last, Some(GrpcAction::Invoke));
}

#[test]
fn picking_a_method_fills_an_empty_message_request() {
    let mut harness = harness(GrpcTab::Message, GrpcSchemaState::Loaded(echo_schema()), "");
    harness.run();
    harness.get_by_label("Select a method").click();
    harness.run();
    harness.get_by_label_contains("Collect").click();
    harness.run();
    assert_eq!(harness.state().draft.method_path, "echo.Echo/Collect");
    assert_eq!(harness.state().last, Some(GrpcAction::GenerateExample));
    assert!(harness.state().dirty);
}

#[test]
fn grpc_authorization_offers_only_supported_kinds() {
    let mut harness = harness(GrpcTab::Authorization, GrpcSchemaState::NotLoaded, "");
    harness.run();
    harness.get_by_label_contains("Inherit").click();
    harness.run();
    assert!(harness.query_by_label_contains("Bearer Token").is_some());
    assert!(harness.query_by_label_contains("OAuth 2.0").is_some());
    assert!(harness.query_by_label_contains("Digest Auth").is_none());
    assert!(harness.query_by_label_contains("AWS Signature v4").is_none());
}
