//! Scripts in the send pipeline: ordering, tests, persistence, failures, pm.sendRequest, the
//! Settings switch and the runner's data scope.

use postguy_engine::{Engine, IterationScope, SendOptions, SendTarget};
use postguy_model::{
    AssertOperator, Assertion, Body, Collection, Environment, Folder, HttpMethod, HttpRequest, Id, Item, KeyValue, RawLanguage, Request, RequestKind,
    RequestTests, Script, Scripts, TestKind, TestStatus, Variable,
};
use postguy_store::StorePaths;
use postguy_testkit::Server;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture { dir: tempfile::tempdir().expect("tempdir") }
    }

    fn engine(&self) -> Engine {
        Engine::open(StorePaths::new(self.dir.path().join("workspace"), self.dir.path().join("config"))).expect("engine")
    }
}

fn scripts(pre: &str, test: &str) -> Scripts {
    let script = |text: &str| (!text.is_empty()).then(|| Script { text: text.into(), ..Script::default() });
    Scripts { prerequest: script(pre), test: script(test), ..Scripts::default() }
}

fn variable(key: &str, value: &str) -> Variable {
    Variable { key: key.into(), value: value.into(), ..Variable::default() }
}

fn row(key: &str, value: &str) -> KeyValue {
    KeyValue { key: key.into(), value: value.into(), ..KeyValue::default() }
}

fn echo(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body).expect("echo json")
}

fn header(echo: &serde_json::Value, name: &str) -> Option<String> {
    echo["headers"]
        .as_array()?
        .iter()
        .find(|h| h["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
        .and_then(|h| h["value"].as_str().map(str::to_string))
}

/// A workspace with environment `env1` (`base` = the server), collection `col1` with one
/// folder and one request `req1`, all scripted by the caller.
struct Rig {
    _fixture: Fixture,
    engine: Engine,
    server: Server,
}

impl Rig {
    fn new(collection_pre: &str, folder_pre: &str, request: HttpRequest) -> Rig {
        let fixture = Fixture::new();
        let engine = fixture.engine();
        let server = Server::start();
        let environment = Environment {
            id: Id("env1".into()),
            name: "dev".into(),
            variables: vec![variable("base", &server.base_url()), variable("id", "1")],
            ..Environment::default()
        };
        engine.workspace().save_environment(&environment).expect("env");
        engine.workspace().set_active_environment(Some(Id("env1".into()))).expect("activate");
        let folder = Folder {
            id: Id("folder1".into()),
            name: "folder".into(),
            scripts: scripts(folder_pre, ""),
            items: vec![Item::Request(Box::new(Request {
                id: Id("req1".into()),
                name: "scripted".into(),
                kind: RequestKind::Http(request),
                ..Request::default()
            }))],
            ..Folder::default()
        };
        let collection = Collection {
            id: Id("col1".into()),
            name: "scripts".into(),
            scripts: scripts(collection_pre, ""),
            items: vec![Item::Folder(Box::new(folder))],
            ..Collection::default()
        };
        engine.workspace().save_collection(&collection).expect("collection");
        Rig { _fixture: fixture, engine, server }
    }

    fn send(&self, options: SendOptions) -> Result<postguy_engine::SendOutcome, postguy_engine::EngineError> {
        self.engine
            .send(SendTarget::Stored { collection: Id("col1".into()), request: Id("req1".into()) }, SendOptions { run_scripts: true, ..options })
    }
}

fn get(url: &str, scripts: Scripts) -> HttpRequest {
    HttpRequest { method: HttpMethod::Get, url: url.into(), scripts, ..HttpRequest::default() }
}

#[test]
fn pre_request_scripts_run_collection_then_folder_then_request() {
    let request = get(
        "{{base}}/echo",
        scripts(
            r#"pm.variables.set("order", pm.variables.get("order") + ",r"); pm.request.headers.add({key: "x-order", value: pm.variables.get("order")});"#,
            "",
        ),
    );
    let rig = Rig::new(r#"pm.variables.set("order", "c");"#, r#"pm.variables.set("order", pm.variables.get("order") + ",f");"#, request);
    let outcome = rig.send(SendOptions::default()).expect("send");
    assert_eq!(header(&echo(&outcome.response.body), "x-order").as_deref(), Some("c,f,r"));
}

#[test]
fn post_response_tests_are_reported_on_the_outcome() {
    let request = get(
        "{{base}}/echo",
        scripts(
            "",
            r#"
            pm.test("status is 200", () => pm.response.to.have.status(200));
            pm.test("wrong", () => pm.expect(pm.response.json().method).to.equal("POST"));
            pm.test.skip("later", () => {});
            "#,
        ),
    );
    let rig = Rig::new("", "", request);
    let outcome = rig.send(SendOptions::default()).expect("send");
    let tests: Vec<_> = outcome.script.tests.iter().map(|t| (t.name.as_str(), t.status, t.message.as_deref())).collect();
    assert_eq!(
        tests,
        vec![
            ("status is 200", TestStatus::Passed, None),
            ("wrong", TestStatus::Failed, Some("expected 'GET' to equal 'POST'")),
            ("later", TestStatus::Skipped, None),
        ]
    );
    assert!(outcome.script.failures.is_empty());
    assert_eq!(rig.engine.response(None).map(|cached| cached.script.tests.len()), Some(3));
}

#[test]
fn script_variable_writes_persist_to_environment_globals_and_collection() {
    let request = get(
        "{{base}}/echo",
        scripts(
            "",
            r#"
            pm.environment.set("token", "abc");
            pm.environment.unset("id");
            pm.globals.set("g", "global");
            pm.collectionVariables.set("c", "coll");
            "#,
        ),
    );
    let rig = Rig::new("", "", request);
    let outcome = rig.send(SendOptions::default()).expect("send");
    assert!(outcome.script.variables_changed);
    let env = rig.engine.workspace().load_environment(&Id("env1".into())).expect("env").into_value();
    let keys: Vec<(&str, &str)> = env.variables.iter().map(|v| (v.key.as_str(), v.value.as_str())).collect();
    assert!(keys.contains(&("token", "abc")), "{keys:?}");
    assert!(!keys.iter().any(|(k, _)| *k == "id"), "{keys:?}");
    let globals = rig.engine.workspace().load_globals().expect("globals").into_value();
    assert!(globals.variables.iter().any(|v| v.key == "g" && v.value == "global"));
    let collection = rig.engine.workspace().load_collection(&Id("col1".into())).expect("collection").into_value();
    assert!(collection.variables.iter().any(|v| v.key == "c" && v.value == "coll"));
    assert_eq!(collection.items.len(), 1, "the collection tree survived the save");
}

#[test]
fn a_failing_pre_request_script_sends_nothing() {
    let request = get("{{base}}/echo", scripts("pm.variables.set('a', 1);\nnull.boom;", ""));
    let rig = Rig::new("", "", request);
    let error = rig.send(SendOptions::default()).expect_err("pre-request failure");
    let text = error.to_string();
    assert!(text.starts_with("pre-request script in request scripted failed at line 2:"), "{text}");
    assert!(rig.engine.history(10).expect("history").is_empty());
    assert!(rig.engine.console_entries(0).iter().any(|entry| entry.message.contains("pre-request script in request scripted")));
}

#[test]
fn skip_request_and_bad_methods_are_reported() {
    let rig = Rig::new("", "", get("{{base}}/echo", scripts("pm.execution.skipRequest();", "")));
    let error = rig.send(SendOptions::default()).expect_err("skipped");
    assert!(matches!(error, postguy_engine::EngineError::Skipped { .. }), "{error}");
    assert!(rig.engine.history(10).expect("history").is_empty());

    let rig = Rig::new("", "", get("{{base}}/echo", scripts("pm.request.method = 'BREW';", "")));
    let error = rig.send(SendOptions::default()).expect_err("bad method");
    assert!(error.to_string().contains("`BREW` is not a valid method"), "{error}");
}

#[test]
fn pre_request_edits_reach_the_wire() {
    let mut request = get(
        "{{base}}/echo?keep=1",
        scripts(
            "pm.request.method = 'POST'; pm.request.url.query.add({key: 'added', value: 'yes'}); pm.request.body.update('{\"n\":1}'); pm.request.headers.upsert({key: 'x-edited', value: 'ok'});",
            "",
        ),
    );
    request.body = Body::Raw { language: RawLanguage::Json, text: "{}".into(), extras: Default::default() };
    request.params = vec![row("fromTable", "t")];
    let rig = Rig::new("", "", request);
    let outcome = rig.send(SendOptions::default()).expect("send");
    let echoed = echo(&outcome.response.body);
    assert_eq!(echoed["method"], "POST");
    assert!(
        echoed["query"].as_str().is_some_and(|q| q.contains("keep=1") && q.contains("fromTable=t") && q.contains("added=yes")),
        "{}",
        echoed["query"]
    );
    assert_eq!(echoed["body"]["utf8"], "{\"n\":1}");
    assert_eq!(header(&echoed, "x-edited").as_deref(), Some("ok"));
}

#[test]
fn pm_send_request_goes_through_the_engine() {
    let request = get(
        "{{base}}/echo",
        scripts(
            r#"
            pm.sendRequest("{{base}}/echo/inner", (err, res) => {
              pm.variables.set("inner", err ? "error" : res.json().path);
            });
            pm.request.headers.add({key: "x-inner", value: pm.variables.get("inner")});
            "#,
            "",
        ),
    );
    let rig = Rig::new("", "", request);
    let outcome = rig.send(SendOptions::default()).expect("send");
    assert_eq!(header(&echo(&outcome.response.body), "x-inner").as_deref(), Some("/echo/inner"));
    assert_eq!(rig.engine.history(10).expect("history").len(), 2, "the nested request is in history too");
    drop(rig.server);
}

#[test]
fn disabled_scripts_are_skipped_with_a_warning() {
    let request = get("{{base}}/echo", scripts("pm.request.headers.add({key: 'x-ran', value: '1'});", "pm.test('t', () => {});"));
    let rig = Rig::new("", "", request);
    let mut settings = rig.engine.settings();
    settings.disable_scripts = true;
    rig.engine.update_settings(settings).expect("settings");
    let outcome = rig.send(SendOptions::default()).expect("send");
    assert!(header(&echo(&outcome.response.body), "x-ran").is_none());
    assert!(outcome.script.tests.is_empty());
    assert!(outcome.warnings.iter().any(|w| w.contains("scripts are disabled in Settings")), "{:?}", outcome.warnings);
}

#[test]
fn data_scope_resolves_between_environment_and_locals() {
    let request = get("{{base}}/echo/{{id}}", scripts("", ""));
    let rig = Rig::new("", "", request);
    let data = IterationScope { index: 0, count: 2, data: vec![("id".into(), serde_json::json!(7))] };
    let outcome = rig.send(SendOptions { iteration: Some(data.clone()), ..SendOptions::default() }).expect("send");
    assert_eq!(echo(&outcome.response.body)["path"], "/echo/7", "data overrides the environment");
    let without = rig.send(SendOptions::default()).expect("send");
    assert_eq!(echo(&without.response.body)["path"], "/echo/1");

    let request = get("{{base}}/echo/{{id}}", scripts("pm.variables.set('id', 'local');", ""));
    let rig = Rig::new("", "", request);
    let local = rig.send(SendOptions { iteration: Some(data), ..SendOptions::default() }).expect("send");
    assert_eq!(echo(&local.response.body)["path"], "/echo/local", "locals override data");
}

#[test]
fn runtime_vars_survive_between_requests_in_a_run() {
    let request = get("{{base}}/echo/{{k}}", scripts("", "pg.setVar('k', 'v1');"));
    let rig = Rig::new("", "", request);
    let first = rig.send(SendOptions::default()).expect("send");
    assert_eq!(echo(&first.response.body)["path"], "/echo/{{k}}", "not set yet");
    assert!(!first.script.variables_changed, "runtime variables never reach a file");
    let second = rig.send(SendOptions::default()).expect("send");
    assert_eq!(echo(&second.response.body)["path"], "/echo/v1", "the runtime scope resolves {{k}}");
    let held = rig.engine.runtime_variables(&Id("col1".into()));
    assert_eq!(held.get("k").map(String::as_str), Some("v1"));
    rig.engine.clear_runtime_variables(&Id("col1".into()));
    assert!(rig.engine.runtime_variables(&Id("col1".into())).is_empty());
}

#[test]
fn request_tests_alone_run_the_post_phase() {
    let mut request = get("{{base}}/echo", scripts("", ""));
    request.tests = RequestTests {
        assertions: vec![
            Assertion { enabled: true, expr: "res.status".into(), operator: AssertOperator::Eq, value: "200".into(), ..Assertion::default() },
            Assertion { enabled: false, expr: "res.status".into(), operator: AssertOperator::Eq, value: "500".into(), ..Assertion::default() },
        ],
        script: Some(Script { text: "pg.report.set('who', 'tests'); pg.test('script ran', () => {});".into(), ..Script::default() }),
        ..RequestTests::default()
    };
    let rig = Rig::new("", "", request);
    let outcome = rig.send(SendOptions::default()).expect("send");
    let kinds: Vec<(TestKind, &str)> = outcome.script.tests.iter().map(|t| (t.kind, t.name.as_str())).collect();
    assert_eq!(kinds, [(TestKind::Assertion, "res.status eq 200"), (TestKind::Test, "script ran")], "{:?}", outcome.script);
    assert!(outcome.script.tests.iter().all(|t| t.status == TestStatus::Passed));
    assert_eq!(outcome.script.report.get("who").and_then(|v| v.as_str()), Some("tests"));
}

fn asset_file(rig: &Rig, name: &str, bytes: &[u8]) {
    let source = rig._fixture.dir.path().join("src").join(name);
    std::fs::create_dir_all(source.parent().expect("parent")).expect("src dir");
    std::fs::write(&source, bytes).expect("source asset");
    let outcome = rig.engine.import_assets(&Id("col1".into()), &[source], "", false).expect("import");
    assert!(outcome.skipped.is_empty(), "{:?}", outcome.skipped);
}

#[test]
fn shortcodes_expand_in_raw_formdata_and_binary_bodies() {
    use base64::Engine as _;
    let mut request = get("{{base}}/echo", scripts("", ""));
    request.method = HttpMethod::Post;
    request.body = Body::Raw {
        language: RawLanguage::Json,
        text: r#"{"image": "{{% base64(file('pic.bin')) %}}", "v": "{{id}}", "t": "{{% text('note.txt') %}}"}"#.into(),
        extras: Default::default(),
    };
    let rig = Rig::new("", "", request.clone());
    let png = [0xffu8, 0x00, 0x10, 0x20];
    asset_file(&rig, "pic.bin", &png);
    asset_file(&rig, "note.txt", b"hello");
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);

    let outcome = rig.send(SendOptions::default()).expect("raw send");
    let body = echo(&outcome.response.body);
    assert_eq!(body["body"]["utf8"], format!(r#"{{"image": "{encoded}", "v": "1", "t": "hello"}}"#));

    // Form-data: a file row becomes an asset upload named after the asset's base name; a text
    // row expands.
    let mut form = request.clone();
    form.body = Body::FormData(vec![
        KeyValue { key: "pic".into(), value: "{{% file('pic.bin') %}}".into(), kind: postguy_model::KeyValueKind::File, ..KeyValue::default() },
        row("text", "{{% base64('abc') %}}"),
    ]);
    save_request(&rig, form);
    let outcome = rig.send(SendOptions::default()).expect("form send");
    let body = echo(&outcome.response.body);
    let sent = body["body"]["utf8"].as_str().map(str::to_string).or_else(|| {
        let raw = base64::engine::general_purpose::STANDARD.decode(body["body"]["base64"].as_str()?).ok()?;
        Some(String::from_utf8_lossy(&raw).into_owned())
    });
    let sent = sent.expect("body text");
    assert!(sent.contains(r#"filename="pic.bin""#), "{sent}");
    assert!(sent.contains("YWJj"), "{sent}");

    // Binary body: the asset's bytes are the payload.
    let mut binary = request;
    binary.body = Body::Binary { path: "{{% file('pic.bin') %}}".into(), extras: Default::default() };
    save_request(&rig, binary);
    let outcome = rig.send(SendOptions::default()).expect("binary send");
    let body = echo(&outcome.response.body);
    assert_eq!(body["body"]["base64"], encoded);
}

fn save_request(rig: &Rig, request: HttpRequest) {
    let mut collection = rig.engine.collection(&Id("col1".into())).expect("collection");
    if let Some(Item::Folder(folder)) = collection.items.first_mut()
        && let Some(Item::Request(stored)) = folder.items.first_mut()
    {
        stored.kind = RequestKind::Http(request);
    }
    rig.engine.workspace().save_collection(&collection).expect("save");
}

#[test]
fn shortcode_errors_abort_the_send_with_the_reason() {
    let mut request = get("{{base}}/echo", scripts("", ""));
    request.method = HttpMethod::Post;
    request.body = Body::Raw { language: RawLanguage::Text, text: "{{% base64(file('missing.png')) %}}".into(), extras: Default::default() };
    let rig = Rig::new("", "", request);
    let err = rig.send(SendOptions::default()).expect_err("missing asset");
    let text = err.to_string();
    assert!(text.contains("body shortcode:") && text.contains("missing.png") && text.contains("Assets tab"), "{text}");
}

#[test]
fn inline_send_with_shortcode_is_actionable_error() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let server = Server::start();
    let request = HttpRequest {
        method: HttpMethod::Post,
        url: format!("{}/echo", server.base_url()),
        body: Body::Raw { language: RawLanguage::Text, text: "{{% base64('x') %}}".into(), extras: Default::default() },
        ..HttpRequest::default()
    };
    let err =
        engine.send(SendTarget::Inline { request: Box::new(request), name: "inline".into() }, SendOptions::default()).expect_err("inline shortcode");
    assert!(err.to_string().contains("save this request in a collection to use {{% %}} shortcodes"), "{err}");
}
