//! Scripts in the send pipeline: ordering, tests, persistence, failures, pm.sendRequest, the
//! Settings switch and the runner's data scope.

use postguy_engine::{Engine, IterationScope, SendOptions, SendTarget};
use postguy_model::{
    Body, Collection, Environment, Folder, HttpMethod, HttpRequest, Id, Item, KeyValue, RawLanguage, Request, RequestKind, Script, Scripts,
    TestStatus, Variable,
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
