//! `postguy-cli run --collection` through the REAL binary against the loopback testkit: the
//! summary, the exit codes an agent or CI branches on, and the written reports.

use std::path::Path;
use std::process::{Command, Output};

use postguy_engine::Engine;
use postguy_model::{Collection, Environment, HttpMethod, HttpRequest, Id, Item, Request, RequestKind, Script, Scripts, Variable};
use postguy_store::StorePaths;
use postguy_testkit::Server;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_postguy-cli"))
}

/// A workspace `<dir>/work` with environment `dev` (base = the server) and a collection
/// `Smoke` holding one GET whose post-response script is `test`.
fn workspace(dir: &Path, server: &Server, test: &str) {
    let engine = Engine::open(StorePaths::new(dir.join("work"), dir.join("config"))).expect("engine");
    let environment = Environment {
        id: Id("env1".into()),
        name: "dev".into(),
        variables: vec![Variable { key: "base".into(), value: server.base_url(), ..Variable::default() }],
        ..Environment::default()
    };
    engine.workspace().save_environment(&environment).expect("env");
    engine.workspace().set_active_environment(Some(Id("env1".into()))).expect("activate");
    let request = Request {
        id: Id("req1".into()),
        name: "Echo".into(),
        kind: RequestKind::Http(HttpRequest {
            method: HttpMethod::Get,
            url: "{{base}}/echo".into(),
            scripts: Scripts { test: Some(Script { text: test.into(), ..Script::default() }), ..Scripts::default() },
            ..HttpRequest::default()
        }),
        ..Request::default()
    };
    let collection =
        Collection { id: Id("col1".into()), name: "Smoke".into(), items: vec![Item::Request(Box::new(request))], ..Collection::default() };
    engine.workspace().save_collection(&collection).expect("collection");
}

fn run(dir: &Path, extra: &[&str]) -> Output {
    cli().args(["run", "--workspace", "work", "--root"]).arg(dir).args(extra).output().expect("run the cli")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn written(dir: &Path, folder: &str, extension: &str) -> String {
    let entry = std::fs::read_dir(dir.join(folder))
        .expect("report folder")
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().ends_with(extension))
        .expect("report file");
    std::fs::read_to_string(entry.path()).expect("report text")
}

#[test]
fn run_collection_prints_the_summary_and_exits_zero() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    workspace(dir.path(), &server, "pm.test('echoes', () => pm.response.to.have.status(200));");
    let output = run(dir.path(), &["--collection", "Smoke", "--iterations", "2", "--reporters", "cli,junit", "--out", "reports"]);
    assert!(output.status.success(), "stdout: {}\nstderr: {}", stdout(&output), stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Iteration 1/2") && text.contains("Iteration 2/2"), "{text}");
    assert!(text.contains("✓ echoes"), "{text}");
    assert!(text.contains("executed") && text.contains("assertions"), "{text}");
    assert!(written(dir.path(), "reports", ".xml").contains("<testsuites"));
}

#[test]
fn a_failing_test_exits_one_and_writes_junit() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    workspace(dir.path(), &server, "pm.test('wrong', () => pm.response.to.have.status(201));");
    let output = run(dir.path(), &["--collection", "col1", "--environment", "dev", "--reporters", "cli,junit,html,json", "--out", "out"]);
    assert_eq!(output.status.code(), Some(1), "stdout: {}\nstderr: {}", stdout(&output), stderr(&output));
    assert!(stdout(&output).contains("✗ wrong: expected response to have status code 201 but got 200"), "{}", stdout(&output));
    assert!(written(dir.path(), "out", ".xml").contains("<failure message=\"expected response to have status code 201 but got 200\"/>"));
    assert!(written(dir.path(), "out", ".html").contains("FAIL wrong"));
    assert!(written(dir.path(), "out", ".json").contains("\"testsFailed\": 1"));
}

#[test]
fn bail_stops_the_run_and_still_exits_one() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    workspace(dir.path(), &server, "pm.test('wrong', () => pm.expect(1).to.equal(2));");
    let output = run(dir.path(), &["--collection", "Smoke", "--iterations", "3", "--bail"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    assert!(text.contains("stopped: stopped after Echo failed (stop on error)"), "{text}");
    assert!(!text.contains("Iteration 2/"), "{text}");
}

#[test]
fn conflicting_run_modes_and_bad_names_exit_two() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    workspace(dir.path(), &server, "");
    let both = cli()
        .args(["run", "--script", "x.json", "--collection", "Smoke", "--workspace", "work"])
        .arg("--root")
        .arg(dir.path())
        .output()
        .expect("both");
    assert_eq!(both.status.code(), Some(2));
    assert!(stderr(&both).contains("run needs either --script FILE or --collection ID --workspace DIR"), "{}", stderr(&both));
    let neither = cli().args(["run", "--root"]).arg(dir.path()).output().expect("neither");
    assert_eq!(neither.status.code(), Some(2));
    let missing_workspace = cli().args(["run", "--collection", "Smoke", "--root"]).arg(dir.path()).output().expect("no workspace");
    assert_eq!(missing_workspace.status.code(), Some(2));

    let unknown = run(dir.path(), &["--collection", "Nope"]);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(stderr(&unknown).contains("no collection `Nope`; known: Smoke (col1)"), "{}", stderr(&unknown));
    let reporter = run(dir.path(), &["--collection", "Smoke", "--reporters", "pdf"]);
    assert_eq!(reporter.status.code(), Some(2));
    let number = run(dir.path(), &["--collection", "Smoke", "--iterations", "many"]);
    assert_eq!(number.status.code(), Some(2));
}

#[test]
fn run_data_files_and_confinement_follow_root() {
    let server = Server::start();
    let dir = tempfile::tempdir().expect("tempdir");
    workspace(dir.path(), &server, "pm.test('row', () => pm.expect(pm.iterationData.get('n')).to.be.a('string'));");
    std::fs::write(dir.path().join("rows.csv"), "n\n1\n2\n3\n").expect("data");
    let output = run(dir.path(), &["--collection", "Smoke", "--data", "rows.csv"]);
    assert!(output.status.success(), "{}{}", stdout(&output), stderr(&output));
    assert!(stdout(&output).contains("Iteration 3/3"), "{}", stdout(&output));
    let outside = run(dir.path(), &["--collection", "Smoke", "--data", "../rows.csv"]);
    assert_eq!(outside.status.code(), Some(1));
    assert!(stderr(&outside).contains("is outside the allowed directory"), "{}", stderr(&outside));
}
