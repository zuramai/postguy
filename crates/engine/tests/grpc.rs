//! The gRPC pipeline: scripts and tests around a call, draft auth, schema loading by reflection and
//! the single-request proto pick.

use postguy_engine::{Engine, GrpcSendOptions};
use postguy_model::auth::BearerAuth;
use postguy_model::{
    AssertOperator, Assertion, AuthConfig, Collection, GrpcRequest, Id, Item, Request, RequestKind, RequestTests, Script, Scripts, ServiceDefinition,
    TestStatus,
};
use postguy_store::StorePaths;
use postguy_testkit::GrpcEchoServer;

const ECHO_PROTO: &str = r#"syntax = "proto3";
package echo;
service Echo {
  rpc Say (EchoMessage) returns (EchoMessage);
  rpc ServerStream (EchoMessage) returns (stream EchoMessage);
  rpc ClientStream (stream EchoMessage) returns (EchoMessage);
  rpc Bidi (stream EchoMessage) returns (stream EchoMessage);
}
message EchoMessage { string text = 1; int32 number = 2; }
"#;

struct Rig {
    dir: tempfile::TempDir,
    engine: Engine,
    server: GrpcEchoServer,
}

fn script(text: &str) -> Option<Script> {
    Some(Script { text: text.into(), ..Script::default() })
}

impl Rig {
    /// A workspace with `protos/echo/echo.proto` and collection `col1` holding `req1`, stored as `stored`.
    fn new(stored: impl FnOnce(&str) -> GrpcRequest) -> Rig {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = Engine::open(StorePaths::new(dir.path().join("workspace"), dir.path().join("config"))).expect("engine");
        let proto_dir = dir.path().join("workspace/protos/echo");
        std::fs::create_dir_all(&proto_dir).expect("proto dir");
        std::fs::write(proto_dir.join("echo.proto"), ECHO_PROTO).expect("proto");
        let server = GrpcEchoServer::start().expect("grpc server");
        let request = stored(server.url());
        let collection = Collection {
            id: Id("col1".into()),
            name: "grpc".into(),
            items: vec![Item::Request(Box::new(Request {
                id: Id("req1".into()),
                name: "say".into(),
                kind: RequestKind::Grpc(request),
                ..Request::default()
            }))],
            ..Collection::default()
        };
        engine.workspace().save_collection(&collection).expect("collection");
        Rig { dir, engine, server }
    }

    fn definition() -> Option<ServiceDefinition> {
        Some(ServiceDefinition::ProtoSet { name: "echo".into(), extras: Default::default() })
    }

    fn send(&self, draft: &GrpcRequest) -> Result<postguy_engine::GrpcSendOutcome, postguy_engine::EngineError> {
        self.engine.grpc_send(&Id("col1".into()), &Id("req1".into()), draft, GrpcSendOptions { run_scripts: true, ..GrpcSendOptions::default() })
    }
}

fn say(url: &str, message: &str) -> GrpcRequest {
    GrpcRequest {
        url: url.into(),
        service_definition: Rig::definition(),
        method_path: "echo.Echo/Say".into(),
        message: message.into(),
        ..GrpcRequest::default()
    }
}

#[test]
fn grpc_send_runs_pre_invoke_on_message_post_response_and_tests() {
    let rig = Rig::new(|url| say(url, "{}"));
    let mut draft = say(rig.server.url(), r#"{"text":"{{t}}"}"#);
    draft.scripts = Scripts {
        before_invoke: script(r#"pm.variables.set("t", "hi");"#),
        on_message: script(r#"pm.test("msg", () => pm.expect(pm.response.json().text).to.equal("hi"));"#),
        ..Scripts::default()
    };
    draft.tests = RequestTests {
        assertions: vec![Assertion {
            enabled: true,
            expr: "res.status".into(),
            operator: AssertOperator::Eq,
            value: "0".into(),
            ..Assertion::default()
        }],
        script: script(r#"pm.test("ok", () => pm.expect(pm.response.code).to.equal(0));"#),
        ..RequestTests::default()
    };
    let outcome = rig.send(&draft).expect("send");
    assert_eq!(outcome.status.name, "OK");
    assert_eq!(outcome.messages.first().map(|m| m["text"].clone()), Some(serde_json::json!("hi")), "{outcome:?}");
    let names: Vec<_> = outcome.script.tests.iter().map(|t| (t.name.as_str(), t.status)).collect();
    assert_eq!(outcome.script.tests.len(), 3, "{names:?} {:?}", outcome.script.failures);
    assert!(outcome.script.tests.iter().all(|t| t.status == TestStatus::Passed), "{:?}", outcome.script.tests);
    assert!(outcome.script.failures.is_empty(), "{:?}", outcome.script.failures);
}

#[test]
fn grpc_send_reports_non_ok_status_and_still_runs_tests() {
    let rig = Rig::new(|url| say(url, "{}"));
    let mut draft = say(rig.server.url(), r#"{"text":"fail"}"#);
    draft.tests =
        RequestTests { script: script(r#"pm.test("not ok", () => pm.expect(pm.response.code !== 0).to.equal(true));"#), ..RequestTests::default() };
    let outcome = rig.send(&draft).expect("a non-OK status is a result");
    assert_ne!(outcome.status.code, 0);
    assert_ne!(outcome.status.name, "OK");
    assert!(outcome.messages.is_empty());
    assert_eq!(outcome.script.tests.len(), 1, "{:?}", outcome.script);
    assert_eq!(outcome.script.tests[0].status, TestStatus::Passed, "{:?}", outcome.script.tests);
}

#[test]
fn grpc_send_uses_unsaved_draft_auth() {
    let rig = Rig::new(|url| GrpcRequest { auth: AuthConfig::NoAuth, ..say(url, "{}") });
    let draft =
        GrpcRequest { auth: AuthConfig::Bearer(Box::new(BearerAuth { token: "x".into(), ..BearerAuth::default() })), ..say(rig.server.url(), "{}") };
    let prepared = rig.engine.prepare_grpc(&Id("col1".into()), &Id("req1".into()), &draft).expect("prepare");
    assert!(prepared.metadata.iter().any(|row| row.key.eq_ignore_ascii_case("authorization") && row.value == "Bearer x"), "{:?}", prepared.metadata);
}

#[test]
fn grpc_schema_lists_services_by_reflection_without_method() {
    let rig = Rig::new(|url| say(url, "{}"));
    let draft = GrpcRequest { url: rig.server.url().into(), service_definition: None, method_path: String::new(), ..GrpcRequest::default() };
    let schema = rig.engine.grpc_schema(&Id("col1".into()), &Id("req1".into()), &draft).expect("schema");
    assert_eq!(schema.services.len(), 1);
    assert_eq!(schema.services[0].name, "echo.Echo");
    assert_eq!(schema.services[0].methods.len(), 4);
    assert_eq!(schema.messages["echo.EchoMessage"].fields.len(), 2);
}

#[test]
fn grpc_schema_reports_reflection_unavailable() {
    let rig = Rig::new(|url| say(url, "{}"));
    let draft = GrpcRequest { url: "http://127.0.0.1:1".into(), service_definition: None, ..GrpcRequest::default() };
    let error = rig.engine.grpc_schema(&Id("col1".into()), &Id("req1".into()), &draft).expect_err("nothing listens on port 1");
    assert!(error.to_string().to_lowercase().contains("connect"), "{error}");
}

#[test]
fn grpc_send_without_a_method_asks_for_one() {
    let rig = Rig::new(|url| say(url, "{}"));
    let draft = GrpcRequest { method_path: String::new(), ..say(rig.server.url(), "{}") };
    let error = rig.send(&draft).expect_err("no method");
    assert!(error.to_string().contains("choose a method first"), "{error}");
}

#[test]
fn grpc_send_cancelled_handle_returns_cancelled() {
    let rig = Rig::new(|url| say(url, "{}"));
    let cancel = postguy_engine::new_cancel();
    cancel.cancel();
    let error = rig
        .engine
        .grpc_send(
            &Id("col1".into()),
            &Id("req1".into()),
            &say(rig.server.url(), "{}"),
            GrpcSendOptions { cancel: Some(cancel), run_scripts: true, ..GrpcSendOptions::default() },
        )
        .expect_err("cancelled");
    assert!(matches!(error, postguy_engine::EngineError::Http(postguy_http::HttpError::Cancelled)), "{error:?}");
}

#[test]
fn import_proto_definition_copies_set_without_collection() {
    let rig = Rig::new(|url| say(url, "{}"));
    let source = rig.dir.path().join("picked");
    std::fs::create_dir_all(&source).expect("source dir");
    std::fs::write(source.join("common.proto"), "syntax = \"proto3\"; package pick; message Shared { string id = 1; }").expect("common");
    std::fs::write(
        source.join("api.proto"),
        "syntax = \"proto3\"; package pick; import \"common.proto\"; service Api { rpc Get (Shared) returns (Shared); }",
    )
    .expect("api");
    let before = rig.engine.collections().expect("collections").len();
    let outcome = rig.engine.import_proto_definition(&[source.join("api.proto")]).expect("import");
    assert_eq!(outcome.definition, ServiceDefinition::ProtoSet { name: outcome.set.clone(), extras: Default::default() });
    let set_dir = rig.dir.path().join("workspace/protos").join(&outcome.set);
    assert!(set_dir.join("api.proto").is_file() && set_dir.join("common.proto").is_file(), "{:?}", outcome.files);
    assert_eq!(rig.engine.collections().expect("collections").len(), before);
}
