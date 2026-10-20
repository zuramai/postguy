//! Engine integration: the twelve-step pipeline against the testkit — inline and stored
//! requests, scope resolution, inherited auth, path/query building, cookies across sessions,
//! history/console recording, secret masking, cancellation and the response cap.

use std::path::PathBuf;

use base64::Engine as _;

use postguy_engine::{Engine, SendOptions, SendTarget};
use postguy_model::auth::BasicAuth;
use postguy_model::{
    AuthConfig, Body, Collection, Environment, Folder, HttpMethod, HttpRequest, Id, Item, KeyValue, RawLanguage, Request, RequestKind, Script,
    Scripts, Variable,
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

    fn paths(&self) -> StorePaths {
        StorePaths::new(self.dir.path().join("workspace"), self.dir.path().join("config"))
    }

    fn engine(&self) -> Engine {
        Engine::open(self.paths()).expect("engine")
    }
}

fn echo_json(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body).expect("echo json")
}

fn header_of(echo: &serde_json::Value, name: &str) -> Option<String> {
    echo["headers"].as_array().and_then(|headers| {
        headers
            .iter()
            .find(|header| header["name"].as_str().is_some_and(|candidate| candidate.eq_ignore_ascii_case(name)))
            .and_then(|header| header["value"].as_str().map(str::to_string))
    })
}

fn env_with(id: &str, variables: Vec<Variable>) -> Environment {
    Environment { id: Id(id.into()), name: id.into(), variables, ..Environment::default() }
}

fn variable(key: &str, value: &str, secret: bool) -> Variable {
    Variable { key: key.into(), value: value.into(), secret, ..Variable::default() }
}

fn inline(request: HttpRequest) -> SendTarget {
    SendTarget::Inline { request: Box::new(request), name: "test request".into() }
}

#[test]
fn a_sent_request_records_history_console_and_resolves_scopes() {
    let server = Server::start();
    let fixture = Fixture::new();
    let engine = fixture.engine();

    // Environment + collection scopes; the collection variable is overridden by the environment.
    let environment = env_with("env1", vec![variable("base", &server.base_url(), false), variable("who", "environment", false)]);
    engine.workspace().save_environment(&environment).expect("save env");
    engine.workspace().set_active_environment(Some(Id("env1".into()))).expect("activate");
    let mut collection = Collection { id: Id("col1".into()), name: "collections".into(), ..Collection::default() };
    collection.variables = vec![variable("who", "collection", false)];
    engine.workspace().save_collection(&collection).expect("save collection");

    // A stored request with a folder chain.
    let mut folder = Folder { id: Id("folder1".into()), name: "folder".into(), ..Folder::default() };
    folder.variables = vec![variable("fromFolder", "yes", false)];
    folder.items.push(Item::Request(Box::new(Request {
        id: Id("req1".into()),
        name: "echo".into(),
        kind: RequestKind::Http(HttpRequest {
            method: HttpMethod::Post,
            url: "{{base}}/echo/:id?raw=1".into(),
            path_vars: vec![KeyValue { key: "id".into(), value: "42".into(), enabled: true, ..KeyValue::default() }],
            params: vec![KeyValue { key: "who".into(), value: "{{who}}".into(), enabled: true, ..KeyValue::default() }],
            headers: vec![KeyValue { key: "X-Folder".into(), value: "{{fromFolder}}".into(), enabled: true, ..KeyValue::default() }],
            body: Body::Raw { language: RawLanguage::Json, text: "{\"hello\":\"{{who}}\"}".into(), extras: Default::default() },
            ..HttpRequest::default()
        }),
        ..Request::default()
    })));
    collection.items = vec![Item::Folder(Box::new(folder))];
    engine.workspace().save_collection(&collection).expect("save collection with request");

    let outcome = engine
        .send(
            SendTarget::Stored { collection: Id("col1".into()), request: Id("req1".into()) },
            SendOptions { environment: None, ..SendOptions::default() },
        )
        .expect("send");

    assert_eq!(outcome.response.status, 200, "url={} body={}", outcome.request.url, String::from_utf8_lossy(&outcome.response.body));
    let echo = echo_json(&outcome.response.body);
    assert_eq!(echo["method"], "POST");
    assert_eq!(echo["path"], "/echo/42", "path variable substituted");
    assert_eq!(echo["query"], "raw=1&who=environment", "query params merged; environment wins");
    assert_eq!(header_of(&echo, "X-Folder").as_deref(), Some("yes"), "folder scope resolved");
    assert_eq!(echo["body"]["utf8"], "{\"hello\":\"environment\"}", "variables resolved inside the body");
    assert!(header_of(&echo, "Content-Type").is_some_and(|value| value == "application/json"));
    assert_eq!(outcome.request.method, "POST");
    assert!(outcome.request.url.contains("/echo/42"), "sent URL recorded");

    // History: one entry, as sent, with the response status.
    let history = engine.history(10).expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, Some(200));
    assert_eq!(history[0].request.as_ref().map(RequestKind::protocol), Some("http"));

    // Console: the request summary is there and no error was logged.
    let console = engine.console_entries(0);
    assert!(console.iter().any(|entry| entry.message.contains("POST") && entry.message.contains("200")), "{console:?}");
    assert!(!console.iter().any(|entry| entry.level == postguy_engine::console::ConsoleLevel::Error));
}

#[test]
fn inherited_basic_auth_is_applied_and_secret_values_are_masked() {
    let server = Server::start();
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let environment =
        env_with("env1", vec![variable("base", &server.base_url(), false), variable("user", "alice", false), variable("pass", "hunter2", true)]);
    engine.workspace().save_environment(&environment).expect("save env");
    engine.workspace().set_active_environment(Some(Id("env1".into()))).expect("activate");
    let mut collection = Collection {
        id: Id("col1".into()),
        name: "auth".into(),
        auth: Some(AuthConfig::Basic(Box::new(BasicAuth { username: "{{user}}".into(), password: "{{pass}}".into(), ..BasicAuth::default() }))),
        ..Collection::default()
    };
    collection.items = vec![Item::Request(Box::new(Request {
        id: Id("req1".into()),
        name: "inherits".into(),
        kind: RequestKind::Http(HttpRequest {
            method: HttpMethod::Get,
            url: "{{base}}/echo".into(),
            headers: vec![KeyValue { key: "X-Secret".into(), value: "{{pass}}".into(), ..KeyValue::default() }],
            ..HttpRequest::default()
        }),
        ..Request::default()
    }))];
    engine.workspace().save_collection(&collection).expect("save");
    let outcome =
        engine.send(SendTarget::Stored { collection: Id("col1".into()), request: Id("req1".into()) }, SendOptions::default()).expect("send");
    let echo = echo_json(&outcome.response.body);
    let expected = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(b"alice:hunter2"));
    assert_eq!(
        header_of(&echo, "Authorization").as_deref(),
        Some(expected.as_str()),
        "inherited auth applied; sent headers: {:?}",
        outcome.request.headers
    );

    // The history snapshot is masked; the console is masked.
    let history = engine.history(1).expect("history");
    let snapshot = serde_json::to_string(&history[0]).expect("serialize");
    assert!(!snapshot.contains("hunter2"), "secret value never enters history");
    let console = serde_json::to_string(&engine.console_entries(0)).expect("serialize");
    assert!(!console.contains("hunter2"), "secret value never enters the console");
    assert!(console.contains('•'), "the mask is visible where the secret was");
}

#[test]
fn cookies_survive_a_restart_through_the_persisted_jar() {
    let server = Server::start();
    let fixture = Fixture::new();
    {
        let engine = fixture.engine();
        let request = HttpRequest {
            method: HttpMethod::Get,
            url: format!("{}/cookies/redirect-set?name=persist&value=yes&to=/cookies/echo", server.base_url()),
            ..HttpRequest::default()
        };
        let outcome = engine.send(inline(request), SendOptions::default()).expect("seed cookies");
        let echo = echo_json(&outcome.response.body);
        assert_eq!(echo["cookieHeader"], "persist=yes");
        assert_eq!(engine.cookies().len(), 1);
    }
    // A fresh Engine (the app restarted) reads the same jar file.
    let engine = fixture.engine();
    let outcome = {
        let request = HttpRequest { method: HttpMethod::Get, url: format!("{}/cookies/echo", server.base_url()), ..HttpRequest::default() };
        engine.send(inline(request), SendOptions::default()).expect("send after restart")
    };
    let echo = echo_json(&outcome.response.body);
    assert_eq!(echo["cookieHeader"], "persist=yes", "the cookie was restored from disk");
}

#[test]
fn the_cookie_jar_disable_setting_sends_nothing_and_stores_nothing() {
    let server = Server::start();
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let mut request = HttpRequest {
        method: HttpMethod::Get,
        url: format!("{}/cookies/redirect-set?name=ignored&value=1&to=/cookies/echo", server.base_url()),
        ..HttpRequest::default()
    };
    request.settings.cookie_jar_disabled = true;
    let outcome = engine.send(inline(request), SendOptions::default()).expect("send");
    let echo = echo_json(&outcome.response.body);
    assert_eq!(echo["cookieHeader"], "");
    assert!(engine.cookies().is_empty(), "nothing stored with the jar disabled");
}

#[test]
fn a_pre_cancelled_send_fails_cleanly_and_writes_no_history() {
    let server = Server::start();
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let cancel = postguy_engine::new_cancel();
    cancel.cancel();
    assert!(cancel.is_cancelled(), "the handle reports cancellation");
    let request = HttpRequest { method: HttpMethod::Get, url: format!("{}/slow?ms=500", server.base_url()), ..HttpRequest::default() };
    let error = engine.send(inline(request), SendOptions { cancel: Some(cancel), ..SendOptions::default() }).expect_err("must be cancelled");
    assert!(error.to_string().contains("cancelled"), "{error}");
    assert!(engine.history(10).expect("history").is_empty());
}

#[test]
fn the_response_cap_is_enforced_with_an_actionable_error() {
    let server = Server::start();
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let mut request = HttpRequest { method: HttpMethod::Get, url: format!("{}/huge?mb=2", server.base_url()), ..HttpRequest::default() };
    request.settings.max_response_mb = 1;
    let error = engine.send(inline(request), SendOptions::default()).expect_err("too large");
    assert!(error.to_string().contains("cap"), "{error}");
}

#[test]
fn scripts_are_an_explicit_no_op_hook_not_a_silent_drop() {
    let server = Server::start();
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let mut collection = Collection { id: Id("col1".into()), name: "scripts".into(), ..Collection::default() };
    collection.scripts = Scripts { prerequest: Some(Script { text: "pm.test('x')".into(), ..Script::default() }), ..Scripts::default() };
    let mut request = HttpRequest { method: HttpMethod::Get, url: format!("{}/echo", server.base_url()), ..HttpRequest::default() };
    request.scripts = Scripts { test: Some(Script { text: "pm.test('y')".into(), ..Script::default() }), ..Scripts::default() };
    collection.items = vec![Item::Request(Box::new(Request {
        id: Id("req1".into()),
        name: "scripted".into(),
        kind: RequestKind::Http(request),
        ..Request::default()
    }))];
    engine.workspace().save_collection(&collection).expect("save");
    let outcome =
        engine.send(SendTarget::Stored { collection: Id("col1".into()), request: Id("req1".into()) }, SendOptions::default()).expect("send");
    assert_eq!(outcome.response.status, 200);
    assert!(outcome.warnings.iter().any(|warning| warning.contains("not executed")), "{:?}", outcome.warnings);
    assert!(engine.console_entries(0).iter().any(|entry| entry.message.contains("skipped")));
}

#[test]
fn bad_urls_and_missing_requests_are_actionable_errors() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let request = HttpRequest { method: HttpMethod::Get, url: "{{missing}}/x".into(), ..HttpRequest::default() };
    let error = engine.send(inline(request), SendOptions::default()).expect_err("no host");
    assert!(error.to_string().contains("host"), "{error}");
    let error = engine
        .send(SendTarget::Stored { collection: Id("nope".into()), request: Id("also-nope".into()) }, SendOptions::default())
        .expect_err("not found");
    assert!(error.to_string().contains("not in this workspace"), "{error}");
}

#[test]
fn file_bodies_are_confined_to_the_working_directory() {
    let server = Server::start();
    let fixture = Fixture::new();
    let engine = fixture.engine();
    std::fs::write(engine.store_paths().files_dir().join("payload.txt"), b"from the workspace").expect("write file");
    let request = HttpRequest {
        method: HttpMethod::Post,
        url: format!("{}/echo", server.base_url()),
        body: Body::Binary { path: "payload.txt".into(), extras: Default::default() },
        ..HttpRequest::default()
    };
    let outcome = engine.send(inline(request), SendOptions::default()).expect("send file");
    let echo = echo_json(&outcome.response.body);
    assert_eq!(echo["body"]["utf8"], "from the workspace");

    // A path outside the working directory is refused unless the setting allows it.
    let outside = fixture.dir.path().join("outside.txt");
    std::fs::write(&outside, b"outside").expect("write outside");
    let request = HttpRequest {
        method: HttpMethod::Post,
        url: format!("{}/echo", server.base_url()),
        body: Body::Binary { path: outside.display().to_string(), extras: Default::default() },
        ..HttpRequest::default()
    };
    let error = engine.send(inline(request), SendOptions::default()).expect_err("outside path");
    assert!(error.to_string().contains("outside the working directory"), "{error}");
}

#[test]
fn resolve_variables_uses_the_active_environment_and_globals() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let environment = env_with("env1", vec![variable("name", "postguy", false)]);
    engine.workspace().save_environment(&environment).expect("save");
    engine.workspace().set_active_environment(Some(Id("env1".into()))).expect("activate");
    let mut globals = env_with("globals", vec![variable("edition", "desktop", false)]);
    globals.id = Id("globals".into());
    engine.workspace().save_globals(&globals).expect("globals");
    let resolution = engine.resolve_variables("{{name}} {{edition}} {{nope}}").expect("resolve");
    assert_eq!(resolution.text, "postguy desktop {{nope}}");
}

/// Keep the `PathBuf` import honest (the fixture uses it for the outside-path case).
#[allow(dead_code)]
fn _path_type(_: Option<PathBuf>) {}
