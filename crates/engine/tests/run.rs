//! Collection runs through the engine against the loopback testkit: data iterations, stop on
//! error, `setNextRequest`, stored runs, cancellation and protocol skipping.

use postguy_engine::{Cancel, Engine, RunConfig, RunEvent, load_data_file};
use postguy_model::{Collection, Environment, GrpcRequest, HttpMethod, HttpRequest, Id, Item, Request, RequestKind, Script, Scripts, Variable};
use postguy_store::StorePaths;
use postguy_testkit::Server;

struct Rig {
    _dir: tempfile::TempDir,
    engine: Engine,
    _server: Server,
}

fn http(id: &str, url: &str, test: &str) -> Item {
    let scripts = Scripts { test: (!test.is_empty()).then(|| Script { text: test.into(), ..Script::default() }), ..Scripts::default() };
    Item::Request(Box::new(Request {
        id: Id(id.into()),
        name: id.into(),
        kind: RequestKind::Http(HttpRequest { method: HttpMethod::Get, url: url.into(), scripts, ..HttpRequest::default() }),
        ..Request::default()
    }))
}

fn rig(items: Vec<Item>) -> Rig {
    let dir = tempfile::tempdir().expect("tempdir");
    let engine = Engine::open(StorePaths::new(dir.path().join("workspace"), dir.path().join("config"))).expect("engine");
    let server = Server::start();
    let environment = Environment {
        id: Id("env1".into()),
        name: "dev".into(),
        variables: vec![Variable { key: "base".into(), value: server.base_url(), ..Variable::default() }],
        ..Environment::default()
    };
    engine.workspace().save_environment(&environment).expect("env");
    engine.workspace().set_active_environment(Some(Id("env1".into()))).expect("activate");
    engine
        .workspace()
        .save_collection(&Collection { id: Id("col".into()), name: "runs".into(), items, ..Collection::default() })
        .expect("collection");
    Rig { _dir: dir, engine, _server: server }
}

fn config() -> RunConfig {
    RunConfig { collection: Id("col".into()), ..RunConfig::default() }
}

fn run(rig: &Rig, config: &RunConfig, data: Option<postguy_engine::DataSet>) -> postguy_engine::RunResult {
    rig.engine.run_collection(config, data, &Cancel::new(), &mut |_| {}).expect("run")
}

const PASS: &str = "pm.test('ok', () => pm.response.to.have.status(200));";
const FAIL: &str = "pm.test('bad', () => pm.response.to.have.status(500));";

#[test]
fn a_run_iterates_data_rows_and_records_tests() {
    let rig = rig(vec![http(
        "echo",
        "{{base}}/echo/{{id}}",
        "pm.test('row path', () => pm.expect(pm.response.json().path).to.equal('/echo/' + pm.iterationData.get('id')));\npm.test('iteration', () => pm.expect(pm.info.iteration).to.be.a('number'));",
    )]);
    let data = postguy_runner::parse_data(b"id\n10\n20\n30\n", "ids.csv").expect("data");
    let mut events = Vec::new();
    let result = rig.engine.run_collection(&config(), Some(data), &Cancel::new(), &mut |event| events.push(event.clone())).expect("run");
    assert_eq!(result.iterations.len(), 3);
    let urls: Vec<&str> = result.iterations.iter().map(|i| i.requests[0].url.rsplit('/').next().unwrap_or_default()).collect();
    assert_eq!(urls, ["10", "20", "30"]);
    assert_eq!(result.totals.requests, 3);
    assert_eq!(result.totals.tests_passed, 6);
    assert!(result.passed(), "{result:?}");
    assert_eq!(result.environment_name.as_deref(), Some("dev"));
    assert!(matches!(events.first(), Some(RunEvent::Started { iterations: 3, requests_per_iteration: 1 })));
    assert_eq!(events.iter().filter(|e| matches!(e, RunEvent::Request { .. })).count(), 3);
    assert!(matches!(events.last(), Some(RunEvent::Finished)));
}

#[test]
fn explicit_iterations_beyond_the_data_rows_get_no_row() {
    let rig = rig(vec![http("echo", "{{base}}/echo/x{{id}}", "")]);
    let data = postguy_runner::parse_data(b"id\n1\n", "ids.csv").expect("data");
    let result = run(&rig, &RunConfig { iterations: Some(2), ..config() }, Some(data));
    let paths: Vec<&str> = result.iterations.iter().map(|i| i.requests[0].url.rsplit('/').next().unwrap_or_default()).collect();
    assert_eq!(paths, ["x1", "x{{id}}"], "the second iteration has no row, so the variable stays unresolved");
}

#[test]
fn stop_on_error_stops_after_the_first_failure() {
    let items = vec![http("a", "{{base}}/echo", FAIL), http("b", "{{base}}/echo", PASS)];
    let continuing = run(&rig(items.clone()), &config(), None);
    assert_eq!(continuing.totals.requests, 2);
    assert_eq!(continuing.totals.tests_failed, 1);
    assert!(!continuing.passed());

    let stopping = run(&rig(items), &RunConfig { stop_on_error: true, ..config() }, None);
    assert_eq!(stopping.totals.requests, 1);
    assert_eq!(stopping.stopped.as_deref(), Some("stopped after a failed (stop on error)"));
    assert!(!stopping.passed());
}

#[test]
fn request_errors_are_rows_not_run_failures() {
    let items = vec![http("down", "http://127.0.0.1:1/x", ""), http("up", "{{base}}/echo", PASS)];
    let result = run(&rig(items), &config(), None);
    let first = &result.iterations[0].requests[0];
    assert!(first.error.is_some(), "{first:?}");
    assert_eq!(first.method, "GET");
    assert_eq!(first.url, "http://127.0.0.1:1/x", "an error row still names the request");
    assert_eq!(result.iterations[0].requests[1].status, Some(200), "the run continues");
    assert_eq!(result.totals.failed_requests, 1);
}

#[test]
fn set_next_request_skips_and_stops() {
    let items = vec![
        http("a", "{{base}}/echo", "pm.execution.setNextRequest('c');"),
        http("b", "{{base}}/echo", ""),
        http("c", "{{base}}/echo", "pm.execution.setNextRequest(null);"),
        http("d", "{{base}}/echo", ""),
    ];
    let result = run(&rig(items), &config(), None);
    let names: Vec<&str> = result.iterations[0].requests.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["a", "c"], "b is jumped over and null ends the iteration before d");
    assert!(result.passed());
}

#[test]
fn non_http_requests_are_skipped_and_counted_apart() {
    let grpc = Item::Request(Box::new(Request {
        id: Id("g".into()),
        name: "g".into(),
        kind: RequestKind::Grpc(GrpcRequest::default()),
        ..Request::default()
    }));
    let result = run(&rig(vec![grpc, http("a", "{{base}}/echo", "")]), &config(), None);
    let first = &result.iterations[0].requests[0];
    assert_eq!(first.skipped.as_deref(), Some("only HTTP requests run in the collection runner"));
    assert_eq!(result.totals.skipped_requests, 1);
    assert_eq!(result.totals.requests, 1);
}

#[test]
fn skip_request_in_a_script_is_a_skipped_row() {
    let result = run(&rig(vec![http("a", "{{base}}/echo", "")]), &config(), None);
    assert_eq!(result.totals.skipped_requests, 0);
    let skipping = Item::Request(Box::new(Request {
        id: Id("s".into()),
        name: "s".into(),
        kind: RequestKind::Http(HttpRequest {
            url: "{{base}}/echo".into(),
            scripts: Scripts { prerequest: Some(Script { text: "pm.execution.skipRequest();".into(), ..Script::default() }), ..Scripts::default() },
            ..HttpRequest::default()
        }),
        ..Request::default()
    }));
    let result = run(&rig(vec![skipping]), &config(), None);
    assert_eq!(result.totals.skipped_requests, 1);
    assert!(result.passed());
}

#[test]
fn runs_are_stored_and_pruned_to_twenty() {
    let rig = rig(vec![http("a", "{{base}}/echo", PASS)]);
    let mut last = String::new();
    for _ in 0..22 {
        last = run(&rig, &config(), None).id;
    }
    let col = Id("col".into());
    let runs = rig.engine.runs(&col).expect("runs");
    assert_eq!(runs.len(), 20);
    assert_eq!(runs[0].id, last, "newest first");
    assert!(runs.iter().all(|r| r.passed));
    let stored = rig.engine.run(&col, &last).expect("run");
    assert_eq!(stored.totals.tests_passed, 1);
    assert!(rig.engine.run(&col, "../etc").is_err());
}

#[test]
fn cancel_stops_between_requests() {
    let rig = rig(vec![http("a", "{{base}}/echo", ""), http("b", "{{base}}/echo", ""), http("c", "{{base}}/echo", "")]);
    let cancel = Cancel::new();
    let trigger = cancel.clone();
    let result = rig
        .engine
        .run_collection(&config(), None, &cancel, &mut |event| {
            if matches!(event, RunEvent::Request { .. }) {
                trigger.cancel();
            }
        })
        .expect("run");
    assert_eq!(result.totals.requests, 1);
    assert_eq!(result.stopped.as_deref(), Some("cancelled"));
    assert!(!result.passed());
}

#[test]
fn bad_configs_are_actionable_errors() {
    let rig = rig(vec![http("a", "{{base}}/echo", "")]);
    let cancel = Cancel::new();
    let err = rig.engine.run_collection(&RunConfig { iterations: Some(0), ..config() }, None, &cancel, &mut |_| {}).unwrap_err();
    assert!(err.to_string().contains("iterations must be between 1 and 10000"), "{err}");
    let err = rig.engine.run_collection(&RunConfig { folder: Some(Id("nope".into())), ..config() }, None, &cancel, &mut |_| {}).unwrap_err();
    assert!(err.to_string().contains("folder `nope` was not found"), "{err}");
    let err = rig.engine.run_collection(&RunConfig { collection: Id("nope".into()), ..config() }, None, &cancel, &mut |_| {}).unwrap_err();
    assert!(err.to_string().contains("is not in this workspace"), "{err}");
    let empty = self::rig(vec![]);
    let err = empty.engine.run_collection(&config(), None, &cancel, &mut |_| {}).unwrap_err();
    assert!(err.to_string().contains("no requests to run"), "{err}");
}

#[test]
fn data_files_load_from_disk_with_a_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let csv = dir.path().join("rows.csv");
    std::fs::write(&csv, "a,b\n1,2\n").expect("write");
    assert_eq!(load_data_file(&csv).expect("csv").rows.len(), 1);
    let json = dir.path().join("rows.json");
    std::fs::write(&json, r#"[{"a":1}]"#).expect("write");
    assert_eq!(load_data_file(&json).expect("json").rows[0][0].1, serde_json::json!(1));
    assert!(load_data_file(&dir.path().join("missing.csv")).is_err());
    let huge = dir.path().join("huge.csv");
    std::fs::write(&huge, vec![b'a'; postguy_runner::DATA_FILE_CAP + 1]).expect("write");
    assert!(load_data_file(&huge).is_err());
}
