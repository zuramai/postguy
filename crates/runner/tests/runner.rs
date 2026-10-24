use postguy_model::{Collection, Folder, Id, Item, NextRequest, Request, RequestKind, ScriptFailure, ScriptPhase, TestResult, TestStatus};
use postguy_runner::report;
use postguy_runner::{IterationResult, RequestRun, RunConfig, RunError, RunResult, Sequencer, Totals, parse_data, plan};

fn request(id: &str, name: &str) -> Item {
    Item::Request(Box::new(Request { id: Id(id.into()), name: name.into(), kind: RequestKind::default(), ..Request::default() }))
}

fn folder(id: &str, name: &str, items: Vec<Item>) -> Item {
    Item::Folder(Box::new(Folder { id: Id(id.into()), name: name.into(), items, ..Folder::default() }))
}

fn tree() -> Collection {
    Collection {
        id: Id("c".into()),
        name: "API".into(),
        items: vec![
            request("r1", "first"),
            folder("f1", "users", vec![request("r2", "list"), folder("f2", "admin", vec![request("r3", "ban")]), request("r4", "get")]),
            request("r5", "last"),
        ],
        ..Collection::default()
    }
}

fn names(plan: &[postguy_runner::PlannedRequest]) -> Vec<&str> {
    plan.iter().map(|p| p.name.as_str()).collect()
}

#[test]
fn plan_walks_depth_first_and_respects_a_subset() {
    let collection = tree();
    let all = plan(&collection, None, None).expect("plan");
    assert_eq!(names(&all), ["first", "list", "ban", "get", "last"]);
    assert_eq!(all[2].path, ["users", "admin"]);
    assert_eq!(all[3].path, ["users"], "the path unwinds after a subfolder");
    assert_eq!(all[4].path, Vec::<String>::new());

    let in_folder = plan(&collection, Some(&Id("f1".into())), None).expect("folder plan");
    assert_eq!(names(&in_folder), ["list", "ban", "get"]);
    assert_eq!(in_folder[0].path, ["users"]);

    let subset = plan(&collection, None, Some(&[Id("r5".into()), Id("r1".into())])).expect("subset");
    assert_eq!(names(&subset), ["last", "first"], "the given order wins");

    assert_eq!(plan(&collection, None, Some(&[Id("nope".into())])).unwrap_err(), RunError::NotFound("request `nope`".into()));
    assert_eq!(plan(&collection, Some(&Id("nope".into())), None).unwrap_err(), RunError::NotFound("folder `nope`".into()));
}

#[test]
fn csv_and_json_data_files_parse_with_row_numbers_in_errors() {
    let csv = parse_data(b"\xEF\xBB\xBFid,name\n1,ann\n2,\"bo,b\"\n", "users.csv").expect("csv");
    assert_eq!(csv.rows.len(), 2);
    assert_eq!(csv.rows[1], vec![("id".to_string(), serde_json::json!("2")), ("name".to_string(), serde_json::json!("bo,b"))]);

    let ragged = parse_data(b"a,b\n1,2\n3\n", "x.csv").unwrap_err().to_string();
    assert!(ragged.starts_with("row 2:"), "{ragged}");

    let json = parse_data(br#"[{"id": 1, "tags": ["a"]}, {"id": 2}]"#, "d.json").expect("json");
    assert_eq!(json.rows[0][1].1, serde_json::json!(["a"]), "JSON values keep their type");
    assert!(parse_data(br#"{"id": 1}"#, "d.json").unwrap_err().to_string().contains("array of objects"));
    assert!(parse_data(br#"[1, 2]"#, "d.json").unwrap_err().to_string().starts_with("row 1:"));
    assert!(parse_data(br#"[{"a":1}]"#, "no-extension").is_ok(), "content starting with [ is JSON");
    assert_eq!(parse_data(b"", "empty.csv").expect("empty").rows.len(), 0);
}

#[test]
fn data_caps_are_enforced() {
    let mut big = String::from("a\n");
    big.push_str(&"x\n".repeat(postguy_runner::MAX_ROWS + 1));
    assert!(parse_data(big.as_bytes(), "big.csv").unwrap_err().to_string().contains("more than 100000 rows"));
    let wide = (0..=postguy_runner::MAX_COLUMNS).map(|i| format!("c{i}")).collect::<Vec<_>>().join(",");
    assert!(parse_data(wide.as_bytes(), "wide.csv").unwrap_err().to_string().contains("more than 1000 columns"));
    let huge = vec![b'a'; postguy_runner::DATA_FILE_CAP + 1];
    assert!(parse_data(&huge, "huge.csv").is_err());
}

#[test]
fn iterations_default_to_data_rows_and_reject_zero() {
    let config = RunConfig::default();
    assert_eq!(config.effective_iterations(None).unwrap(), 1);
    assert_eq!(config.effective_iterations(Some(3)).unwrap(), 3);
    assert!(config.effective_iterations(Some(0)).is_err(), "an empty data file runs nothing");
    let explicit = RunConfig { iterations: Some(5), ..RunConfig::default() };
    assert_eq!(explicit.effective_iterations(Some(3)).unwrap(), 5);
    assert!(RunConfig { iterations: Some(0), ..RunConfig::default() }.effective_iterations(None).is_err());
    assert!(RunConfig { iterations: Some(10_001), ..RunConfig::default() }.effective_iterations(None).is_err());
    assert!(RunConfig { delay_ms: 600_001, ..RunConfig::default() }.effective_iterations(None).is_err());
}

#[test]
fn set_next_request_jumps_by_name_stops_on_null_and_caps_loops() {
    let planned = plan(&tree(), None, None).expect("plan");
    let mut sequencer = Sequencer::new(planned.clone());
    assert_eq!(sequencer.start(), Some(0));
    assert_eq!(sequencer.next(&NextRequest::Unchanged), Some(1));
    assert_eq!(sequencer.next(&NextRequest::Named("last".into())), Some(4), "jump forward by name");
    assert_eq!(sequencer.next(&NextRequest::Unchanged), None, "the end of the plan");

    assert_eq!(sequencer.start(), Some(0));
    assert_eq!(sequencer.next(&NextRequest::Named("r3".into())), Some(2), "an id works too");
    assert_eq!(sequencer.next(&NextRequest::Stop), None);

    assert_eq!(sequencer.start(), Some(0));
    assert_eq!(sequencer.next(&NextRequest::Named("ghost".into())), None);
    assert_eq!(sequencer.take_warnings(), ["setNextRequest(\"ghost\") matched no request; the iteration ended"]);

    // A request that always jumps back to itself ends at the step cap.
    assert_eq!(sequencer.start(), Some(0));
    let mut steps = 1;
    while sequencer.next(&NextRequest::Named("first".into())).is_some() {
        steps += 1;
    }
    assert_eq!(steps, postguy_runner::MAX_STEPS_PER_ITERATION);
    assert!(sequencer.take_warnings()[0].contains("looped more than 10000 times"));
}

fn test(name: &str, status: TestStatus, message: Option<&str>) -> TestResult {
    TestResult { name: name.into(), status, message: message.map(str::to_string), phase: ScriptPhase::PostResponse }
}

fn run(name: &str, tests: Vec<TestResult>) -> RequestRun {
    RequestRun {
        request_id: Id("r".into()),
        name: name.into(),
        path: vec!["users".into()],
        method: "GET".into(),
        url: "http://x/".into(),
        status: Some(200),
        duration_ms: 1500,
        size_bytes: 12,
        tests,
        failures: Vec::new(),
        error: None,
        skipped: None,
    }
}

fn result(requests: Vec<RequestRun>) -> RunResult {
    let mut totals = Totals::default();
    for request in &requests {
        totals.add(request);
    }
    RunResult {
        id: "run1".into(),
        collection_id: Id("c".into()),
        collection_name: "API & <co>".into(),
        started_ms: 1000,
        finished_ms: 3500,
        iterations: vec![IterationResult { index: 0, requests, warnings: Vec::new() }],
        totals,
        ..RunResult::default()
    }
}

#[test]
fn junit_escapes_and_counts_failures() {
    let mut bad = run(
        "bad",
        vec![
            test("a \"quoted\" <name>", TestStatus::Failed, Some("expected 1 to equal 2\u{1}")),
            test("ok", TestStatus::Passed, None),
            test("later", TestStatus::Skipped, None),
        ],
    );
    bad.failures.push(ScriptFailure { phase: ScriptPhase::PreRequest, origin: "request bad".into(), message: "boom & more".into(), line: Some(2) });
    let mut broken = run("broken", vec![]);
    broken.error = Some("connection refused".into());
    let xml = report::junit(&result(vec![bad, broken]));
    assert!(xml.contains("<testsuites name=\"API &amp; &lt;co&gt;\" tests=\"3\" failures=\"2\" time=\"2.500\">"), "{xml}");
    assert!(xml.contains("<testsuite name=\"1/users/bad\" tests=\"3\" failures=\"2\" time=\"1.500\">"), "{xml}");
    assert!(xml.contains("<testcase name=\"a &quot;quoted&quot; &lt;name&gt;\"><failure message=\"expected 1 to equal 2\"/></testcase>"), "{xml}");
    assert!(xml.contains("<testcase name=\"later\"><skipped/></testcase>"));
    assert!(xml.contains("<testcase name=\"request\"><error message=\"connection refused\"/></testcase>"));
    assert!(xml.contains("boom &amp; more"));
    assert!(!xml.contains('\u{1}'), "control characters are dropped");
}

#[test]
fn html_has_no_script_and_escapes_user_text() {
    let evil = test("<script>alert(1)</script>", TestStatus::Failed, Some("<img src=x onerror=alert(1)>"));
    let html = report::html(&result(vec![run("<b>req</b>", vec![evil])]));
    assert!(!html.to_ascii_lowercase().contains("<script"), "{html}");
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
    assert!(!html.contains("http://") || !html.contains("src=\"http"), "no external resources");
    assert!(html.contains("<span class=\"fail\">failed</span>"));
}

#[test]
fn cli_summary_counts_match_totals() {
    let r = result(vec![
        run("one", vec![test("p1", TestStatus::Passed, None), test("p2", TestStatus::Passed, None)]),
        run("two", vec![test("f1", TestStatus::Failed, Some("nope")), test("s1", TestStatus::Skipped, None)]),
    ]);
    let text = report::cli(&r);
    assert!(text.contains("Iteration 1/1"), "{text}");
    assert!(text.contains("  ✓ p1") && text.contains("  ✗ f1: nope") && text.contains("  ○ s1 (skipped)"), "{text}");
    assert!(text.contains("GET http://x/ [200, 12 B, 1500 ms]"));
    let row = |label: &str| {
        text.lines()
            .find(|line| line.contains(label))
            .unwrap_or_default()
            .split('│')
            .map(str::trim)
            .filter(|cell| !cell.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert_eq!(row("requests"), ["requests", "2", "1"]);
    assert_eq!(row("assertions"), ["assertions", "3", "1"], "skipped tests are not executed assertions");
    assert_eq!(row("iterations"), ["iterations", "1", "1"]);
    assert!(text.contains("total run duration: 2500 ms"));
    assert!(!r.passed());
    let ok = result(vec![run("fine", vec![test("p", TestStatus::Passed, None)])]);
    assert!(ok.passed());
    assert_eq!(ok.summary().iterations, 1);
}

#[test]
fn a_stopped_run_is_not_a_pass_and_json_round_trips() {
    let mut r = result(vec![run("fine", vec![])]);
    r.stopped = Some("cancelled".into());
    assert!(!r.passed());
    let json = report::json(&r);
    assert_eq!(json["totals"]["requests"], 1);
    assert_eq!(json["stopped"], "cancelled");
    let back: RunResult = serde_json::from_value(json).expect("round trip");
    assert_eq!(back, r);
}
