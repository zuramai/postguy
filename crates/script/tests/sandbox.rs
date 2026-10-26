use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use postguy_model::{AssertOperator, Assertion, NextRequest, ScriptPhase, TestKind, TestStatus};
use postguy_script::{
    Context, Header, Host, Info, Limits, Outcome, ScriptBody, ScriptRequest, ScriptResponse, ScriptSource, TestBlock, Variables, run,
};

struct NoHost;

impl Host for NoHost {
    fn send_request(&self, _: ScriptRequest) -> Result<ScriptResponse, String> {
        Err("no network in tests".to_string())
    }
    fn cookies_for(&self, _: &str) -> Vec<(String, String)> {
        vec![("sid".to_string(), "abc".to_string())]
    }
}

fn request() -> ScriptRequest {
    ScriptRequest {
        id: "r1".into(),
        name: "Login".into(),
        method: "GET".into(),
        url: "https://api.test/users?a=1".into(),
        headers: vec![Header { key: "Accept".into(), value: "text/plain".into(), enabled: true }],
        body: ScriptBody::None,
    }
}

fn response() -> ScriptResponse {
    ScriptResponse {
        code: 200,
        status: "OK".into(),
        headers: vec![("Content-Type".into(), "application/json".into()), ("Set-Cookie".into(), "token=xyz; Path=/".into())],
        body: br#"{"id":7,"name":"ann","tags":["a","b"]}"#.to_vec(),
        response_time_ms: 42,
        size: 40,
    }
}

fn context(phase: ScriptPhase, with_response: bool, variables: Variables) -> Context {
    Context {
        phase,
        info: Info {
            event_name: if phase == ScriptPhase::PreRequest { "prerequest" } else { "test" },
            iteration: 0,
            iteration_count: 1,
            request_name: "Login".into(),
            request_id: "r1".into(),
        },
        variables,
        request: request(),
        response: with_response.then(response),
        host: Arc::new(NoHost),
        tests: None,
    }
}

fn script(text: &str) -> Vec<ScriptSource> {
    vec![ScriptSource { origin: "request Login".into(), text: text.into() }]
}

fn never() -> postguy_script::CancelCheck {
    Arc::new(|| false)
}

fn post(text: &str) -> Outcome {
    run(&script(text), context(ScriptPhase::PostResponse, true, Variables::default()), &Limits::default(), never())
}

fn pre(text: &str, variables: Variables) -> Outcome {
    run(&script(text), context(ScriptPhase::PreRequest, false, variables), &Limits::default(), never())
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn only_failure(outcome: &Outcome) -> String {
    assert_eq!(outcome.failures.len(), 1, "failures: {:?}", outcome.failures);
    outcome.failures[0].message.clone()
}

#[test]
fn pm_test_records_pass_fail_and_skip() {
    let outcome = post(
        r#"
        pm.test("passes", () => { pm.expect(1).to.equal(1); });
        pm.test("fails", () => { pm.expect(1).to.equal(2); });
        pm.test.skip("skipped", () => { throw new Error("never runs"); });
        pm.test("async ok", () => Promise.resolve(1));
        pm.test("async bad", () => Promise.reject(new Error("nope")));
        "#,
    );
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    let got: Vec<(&str, TestStatus, Option<&str>)> = outcome.tests.iter().map(|t| (t.name.as_str(), t.status, t.message.as_deref())).collect();
    assert_eq!(
        got,
        vec![
            ("passes", TestStatus::Passed, None),
            ("fails", TestStatus::Failed, Some("expected 1 to equal 2")),
            ("skipped", TestStatus::Skipped, None),
            ("async ok", TestStatus::Passed, None),
            ("async bad", TestStatus::Failed, Some("nope")),
        ]
    );
    assert!(outcome.tests.iter().all(|t| t.phase == ScriptPhase::PostResponse));
}

#[test]
fn expect_chains_report_chai_wording() {
    let outcome = post(
        r#"
        const cases = [
          () => pm.expect(1).to.equal(2),
          () => pm.expect({a:1}).to.not.have.property('a'),
          () => pm.expect({a:1}).to.have.property('a', 2),
          () => pm.expect('x').to.be.a('number'),
          () => pm.expect(5).to.be.above(9),
          () => pm.expect([1,2]).to.have.lengthOf(3),
          () => pm.expect(3).to.be.within(5, 9),
          () => pm.expect([1]).to.include(2),
          () => pm.expect(null).to.exist,
          () => pm.expect({a:1}).to.eql({a:2}),
          () => pm.expect('abc').to.match(/x/),
          () => pm.expect(3).to.be.oneOf([1, 2]),
          () => pm.expect(true).to.be.false,
        ];
        cases.forEach((c, i) => pm.test("c" + i, c));
        pm.test("ok chain", () => {
          pm.expect({a: {b: 1}}).to.deep.equal({a: {b: 1}});
          pm.expect([1, 2, 3]).to.have.members([3, 2, 1]);
          pm.expect({a: 1, b: 2}).to.have.all.keys('a', 'b');
          pm.expect('hello').to.include('ell').and.have.lengthOf(5);
          pm.expect({a: 1}).to.have.property('a').that.equals(1);
          pm.expect(undefined).to.be.undefined;
          pm.expect(0).to.not.be.ok;
        });
        "#,
    );
    let messages: Vec<String> = outcome.tests.iter().map(|t| t.message.clone().unwrap_or_else(|| "PASS".into())).collect();
    assert_eq!(
        messages,
        vec![
            "expected 1 to equal 2",
            "expected { a: 1 } to not have property 'a'",
            "expected { a: 1 } to have property 'a' of 2, but got 1",
            "expected 'x' to be a number",
            "expected 5 to be above 9",
            "expected [ 1, 2 ] to have a length of 3 but got 2",
            "expected 3 to be within 5..9",
            "expected [ 1 ] to include 2",
            "expected null to exist",
            "expected { a: 1 } to deeply equal { a: 2 }",
            "expected 'abc' to match /x/",
            "expected 3 to be one of [ 1, 2 ]",
            "expected true to be false",
            "PASS",
        ]
    );
}

#[test]
fn response_assertions_check_status_headers_and_json() {
    let outcome = post(
        r#"
        pm.test("status", () => pm.response.to.have.status(200));
        pm.test("wrong status", () => pm.response.to.have.status(404));
        pm.test("negated", () => pm.response.to.not.have.status(200));
        pm.test("header", () => pm.response.to.have.header("content-type", "application/json"));
        pm.test("missing header", () => pm.response.to.have.header("x-nope"));
        pm.test("ok", () => pm.response.to.be.ok);
        pm.test("json", () => pm.response.to.be.json);
        pm.test("server error", () => pm.response.to.be.serverError);
        pm.test("body", () => { pm.expect(pm.response.json().tags).to.have.lengthOf(2); pm.expect(pm.response.text()).to.include("ann"); });
        pm.test("meta", () => { pm.expect(pm.response.code).to.equal(200); pm.expect(pm.response.responseTime).to.equal(42); pm.expect(pm.response.headers.get("CONTENT-TYPE")).to.include("json"); });
        pm.test("cookies", () => { pm.expect(pm.cookies.get("token")).to.equal("xyz"); pm.expect(pm.cookies.get("sid")).to.equal("abc"); });
        "#,
    );
    let got: Vec<(String, bool, String)> =
        outcome.tests.iter().map(|t| (t.name.clone(), t.status == TestStatus::Passed, t.message.clone().unwrap_or_default())).collect();
    let status = |name: &str| got.iter().find(|(n, _, _)| n == name).cloned().unwrap_or_default();
    assert!(status("status").1, "{got:?}");
    assert_eq!(status("wrong status").2, "expected response to have status code 404 but got 200");
    assert_eq!(status("negated").2, "expected response to not have status code 200");
    assert!(status("header").1);
    assert_eq!(status("missing header").2, "expected response to have header x-nope");
    assert!(status("ok").1 && status("json").1 && status("body").1 && status("meta").1 && status("cookies").1, "{got:?}");
    assert!(!status("server error").1);
}

#[test]
fn variables_set_unset_and_precedence() {
    let variables = Variables {
        globals: map(&[("k", "globals"), ("only_g", "g")]),
        collection: map(&[("k", "collection")]),
        folders: map(&[("k", "folder")]),
        environment: Some(("dev".into(), map(&[("k", "env"), ("gone", "x")]))),
        data: vec![("k".into(), serde_json::json!("data")), ("n".into(), serde_json::json!(5))],
        locals: BTreeMap::new(),
        runtime: BTreeMap::new(),
    };
    let outcome = pre(
        r#"
        pm.test("precedence", () => {
          pm.expect(pm.variables.get("k")).to.equal("data");
          pm.expect(pm.iterationData.get("n")).to.equal(5);
          pm.variables.set("k", "local");
          pm.expect(pm.variables.get("k")).to.equal("local");
          pm.variables.unset("k");
          pm.expect(pm.variables.get("k")).to.equal("data");
          pm.expect(pm.variables.get("only_g")).to.equal("g");
          pm.expect(pm.environment.name).to.equal("dev");
          pm.expect(pm.variables.replaceIn("{{k}}-{{only_g}}")).to.equal("data-g");
        });
        pm.environment.set("token", "t1");
        pm.environment.set("num", 42);
        pm.environment.unset("gone");
        pm.globals.set("g2", {a: 1});
        pm.collectionVariables.set("c", "1");
        pm.test("has", () => { pm.expect(pm.environment.has("token")).to.be.true; pm.expect(pm.environment.has("gone")).to.be.false; });
        "#,
        variables,
    );
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert!(outcome.tests.iter().all(|t| t.status == TestStatus::Passed), "{:?}", outcome.tests);
    let env = outcome.variables.environment.unwrap().1;
    assert_eq!(env, map(&[("k", "env"), ("token", "t1"), ("num", "42")]));
    assert_eq!(outcome.variables.globals.get("g2").map(String::as_str), Some(r#"{"a":1}"#));
    assert_eq!(outcome.variables.collection.get("c").map(String::as_str), Some("1"));
    assert!(outcome.variables.locals.is_empty());
}

#[test]
fn environment_set_without_an_environment_is_an_actionable_error() {
    let outcome = pre(r#"pm.environment.set("x", "1");"#, Variables::default());
    assert_eq!(only_failure(&outcome), "no environment is selected; choose one before setting environment variables");
}

#[test]
fn pre_request_mutations_change_url_headers_and_body() {
    let outcome = pre(
        r#"
        pm.request.method = "post";
        pm.request.headers.add({key: "X-A", value: "1"});
        pm.request.headers.upsert({key: "accept", value: "application/json"});
        pm.request.headers.remove("nope");
        pm.request.url.query.add({key: "b", value: "x y"});
        pm.request.url.query.upsert({key: "a", value: "2"});
        pm.request.body.update(JSON.stringify({hello: "world"}));
        console.log(pm.request.url.getHost(), pm.request.url.getPath(), pm.request.method, pm.request.headers.get("ACCEPT"));
        "#,
        Variables::default(),
    );
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    let request = outcome.request;
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, "https://api.test/users?a=2&b=x%20y");
    assert_eq!(request.body, ScriptBody::Raw(r#"{"hello":"world"}"#.into()));
    assert_eq!(request.headers.len(), 2);
    assert_eq!(request.headers[0].value, "application/json");
    assert_eq!(outcome.logs[0].message, "api.test /users POST application/json");
}

#[test]
fn post_response_request_is_read_only() {
    let outcome = post(
        r#"
        pm.test("read only", () => { pm.request.headers.add({key: "a", value: "b"}); });
        pm.test("reads", () => { pm.expect(pm.request.url.toString()).to.include("api.test"); });
        "#,
    );
    assert_eq!(outcome.tests[0].message.as_deref(), Some("pm.request is read-only in post-response scripts"));
    assert_eq!(outcome.tests[1].status, TestStatus::Passed);
}

#[test]
fn set_next_request_and_skip_request_are_reported() {
    let next = post(r#"pm.execution.setNextRequest("Other");"#);
    assert_eq!(next.next_request, NextRequest::Named("Other".into()));
    let stop = post(r#"postman.setNextRequest(null);"#);
    assert_eq!(stop.next_request, NextRequest::Stop);
    let skip = pre(r#"pm.execution.skipRequest();"#, Variables::default());
    assert!(skip.skip_request);
    let bad = post(r#"pm.execution.skipRequest();"#);
    assert!(only_failure(&bad).contains("pre-request"));
}

#[test]
fn console_lines_are_captured_and_capped() {
    let limits = Limits { max_log_lines: 3, ..Limits::default() };
    let outcome = run(
        &script(r#"console.log("a", 1, {b: [2]}); console.warn("w"); console.error("e"); console.info("dropped"); console.log("x".repeat(40000));"#),
        context(ScriptPhase::PostResponse, true, Variables::default()),
        &limits,
        never(),
    );
    assert_eq!(outcome.logs.len(), 3);
    assert_eq!(outcome.logs[0].message, "a 1 { b: [ 2 ] }");
    assert_eq!(format!("{:?}", outcome.logs[1].level), "Warn");

    let long = run(
        &script(r#"console.log("x".repeat(40000));"#),
        context(ScriptPhase::PostResponse, true, Variables::default()),
        &Limits::default(),
        never(),
    );
    assert!(long.logs[0].message.ends_with("… (truncated)"));
    assert!(long.logs[0].message.len() < 20_000);
}

#[test]
fn a_syntax_error_names_the_origin_and_line() {
    let outcome = post("let a = 1;\nlet b = ;\n");
    assert_eq!(outcome.failures.len(), 1);
    let failure = &outcome.failures[0];
    assert_eq!(failure.origin, "request Login");
    assert_eq!(failure.line, Some(2), "{failure:?}");
    assert!(!failure.message.is_empty());
    let runtime_error = post("\n\nnull.x;");
    assert_eq!(runtime_error.failures[0].line, Some(3), "{:?}", runtime_error.failures);
}

#[test]
fn collection_and_request_scripts_share_state_through_variables_not_scope() {
    let sources = vec![
        ScriptSource { origin: "collection C".into(), text: "const shared = 1; pm.variables.set('a', 'from-collection');".into() },
        ScriptSource { origin: "request Login".into(), text: "const shared = 2; pm.variables.set('b', pm.variables.get('a') + '+request');".into() },
    ];
    let outcome = run(&sources, context(ScriptPhase::PreRequest, false, Variables::default()), &Limits::default(), never());
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(outcome.variables.locals.get("b").map(String::as_str), Some("from-collection+request"));
}

#[test]
fn a_failing_pre_request_script_skips_the_rest_but_post_response_continues() {
    let sources = vec![
        ScriptSource { origin: "collection C".into(), text: "throw new Error('boom');".into() },
        ScriptSource { origin: "request Login".into(), text: "pm.variables.set('ran', '1');".into() },
    ];
    let pre = run(&sources, context(ScriptPhase::PreRequest, false, Variables::default()), &Limits::default(), never());
    assert_eq!(pre.failures.len(), 1);
    assert!(pre.variables.locals.is_empty());
    let post = run(&sources, context(ScriptPhase::PostResponse, true, Variables::default()), &Limits::default(), never());
    assert_eq!(post.failures.len(), 1);
    assert_eq!(post.failures[0].origin, "collection C");
    assert_eq!(post.variables.locals.get("ran").map(String::as_str), Some("1"));
}

#[test]
fn send_request_goes_through_the_host() {
    struct Echo;
    impl Host for Echo {
        fn send_request(&self, request: ScriptRequest) -> Result<ScriptResponse, String> {
            if request.url.contains("fail") {
                return Err("connection refused".into());
            }
            Ok(ScriptResponse {
                code: 201,
                status: "Created".into(),
                headers: vec![],
                body: format!("{} {}", request.method, request.url).into_bytes(),
                response_time_ms: 1,
                size: 1,
            })
        }
        fn cookies_for(&self, _: &str) -> Vec<(String, String)> {
            Vec::new()
        }
    }
    let mut ctx = context(ScriptPhase::PreRequest, false, Variables::default());
    ctx.host = Arc::new(Echo);
    let outcome = run(
        &script(
            r#"
            pm.sendRequest({url: "http://x/a", method: "post"}, (err, res) => { pm.variables.set("body", res.text()); pm.variables.set("code", res.code); });
            pm.sendRequest("http://x/fail", (err, res) => { pm.variables.set("err", err.message); });
            pm.sendRequest("http://x/p").then(res => pm.variables.set("promise", String(res.code)));
            "#,
        ),
        ctx,
        &Limits::default(),
        never(),
    );
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    let l = &outcome.variables.locals;
    assert_eq!(l.get("body").map(String::as_str), Some("POST http://x/a"));
    assert_eq!(l.get("code").map(String::as_str), Some("201"));
    assert_eq!(l.get("err").map(String::as_str), Some("connection refused"));
    assert_eq!(l.get("promise").map(String::as_str), Some("201"));

    let limits = Limits { max_send_requests: 2, ..Limits::default() };
    let capped = run(
        &script(r#"for (let i = 0; i < 5; i++) pm.sendRequest("http://x/" + i, () => {});"#),
        context(ScriptPhase::PreRequest, false, Variables::default()),
        &limits,
        never(),
    );
    assert!(only_failure(&capped).contains("more than 2 times"), "{:?}", capped.failures);
}

#[test]
fn timers_btoa_and_promises_settle_before_the_run_ends() {
    let outcome = post(
        r#"
        setTimeout(() => pm.variables.set("late", "2"), 20);
        setTimeout(() => pm.variables.set("early", "1"), 5);
        pm.variables.set("b64", btoa("hi") + atob("aGk="));
        Promise.resolve().then(() => pm.variables.set("micro", "m"));
        "#,
    );
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    let l = outcome.variables.locals;
    assert_eq!(l.get("late").map(String::as_str), Some("2"));
    assert_eq!(l.get("early").map(String::as_str), Some("1"));
    assert_eq!(l.get("b64").map(String::as_str), Some("aGk=hi"));
    assert_eq!(l.get("micro").map(String::as_str), Some("m"));
}

// ---- hostile scripts: these close docs/gaps.md item 8 ----------------------------------------

#[test]
fn an_infinite_loop_stops_at_the_wall_clock() {
    let limits = Limits { timeout_ms: 200, ..Limits::default() };
    let started = Instant::now();
    let outcome = run(&script("while (true) {}"), context(ScriptPhase::PostResponse, true, Variables::default()), &limits, never());
    assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
    assert_eq!(only_failure(&outcome), "the script ran longer than 200 ms");
}

#[test]
fn a_loop_inside_a_test_cannot_swallow_the_timeout() {
    let limits = Limits { timeout_ms: 200, ..Limits::default() };
    let started = Instant::now();
    let outcome = run(
        &script("try { pm.test('spin', () => { for(;;){} }); } catch (e) {} while (true) {}"),
        context(ScriptPhase::PostResponse, true, Variables::default()),
        &limits,
        never(),
    );
    assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
    assert_eq!(only_failure(&outcome), "the script ran longer than 200 ms");
}

#[test]
fn a_memory_bomb_stops_at_the_memory_limit() {
    let limits = Limits { memory_bytes: 16 << 20, timeout_ms: 20_000, ..Limits::default() };
    let outcome = run(
        &script("let a = []; for (;;) a.push(new Array(1e5).fill(1));"),
        context(ScriptPhase::PostResponse, true, Variables::default()),
        &limits,
        never(),
    );
    assert_eq!(only_failure(&outcome), "the script used more than 16 MiB of memory");
}

#[test]
fn deep_recursion_stops_cleanly() {
    let outcome = post("function f() { return f() + 1; } f();");
    assert_eq!(outcome.failures.len(), 1);
    assert!(!outcome.failures[0].message.is_empty());
    let mixed = post("function g(n) { return JSON.stringify([n, g(n + 1)]); } pm.test('deep', () => g(0));");
    assert!(mixed.failures.is_empty() || mixed.failures.len() == 1);
}

#[test]
fn cancel_interrupts_a_running_script() {
    let flag = Arc::new(AtomicBool::new(false));
    let setter = flag.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        setter.store(true, Ordering::Relaxed);
    });
    let check: postguy_script::CancelCheck = Arc::new(move || flag.load(Ordering::Relaxed));
    let started = Instant::now();
    let limits = Limits { timeout_ms: 30_000, ..Limits::default() };
    let outcome = run(&script("while (true) {}"), context(ScriptPhase::PostResponse, true, Variables::default()), &limits, check);
    let _ = handle.join();
    assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());
    assert_eq!(only_failure(&outcome), "the script was cancelled");
}

#[test]
fn a_cyclic_or_huge_value_in_expect_is_an_error_not_a_hang() {
    let outcome = post("const a = {}; a.self = a; pm.test('cycle', () => pm.expect(a).to.equal(1));");
    assert_eq!(outcome.tests[0].message.as_deref(), Some("value is too deeply nested or too large"));
}

#[test]
fn the_test_count_is_capped() {
    let limits = Limits { max_tests: 3, ..Limits::default() };
    let outcome = run(
        &script("for (let i = 0; i < 10; i++) pm.test('t' + i, () => {});"),
        context(ScriptPhase::PostResponse, true, Variables::default()),
        &limits,
        never(),
    );
    assert_eq!(outcome.tests.len(), 3);
    assert_eq!(only_failure(&outcome), "more than 3 tests; the rest were ignored");
}

// ----------------------------------------------------------------------------------------
// Tests tab, pg, Bruno globals, require

struct AssetHost(BTreeMap<String, Vec<u8>>);

impl Host for AssetHost {
    fn send_request(&self, _: ScriptRequest) -> Result<ScriptResponse, String> {
        Err("no network in tests".to_string())
    }
    fn cookies_for(&self, _: &str) -> Vec<(String, String)> {
        Vec::new()
    }
    fn read_asset(&self, path: &str) -> Result<Vec<u8>, String> {
        self.0.get(path).cloned().ok_or_else(|| format!("asset `{path}` does not exist in this collection"))
    }
    fn list_assets(&self, dir: &str) -> Result<Vec<String>, String> {
        let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        let mut names: Vec<String> = Vec::new();
        for key in self.0.keys() {
            if let Some(rest) = key.strip_prefix(&prefix) {
                let name = rest.split('/').next().unwrap_or(rest).to_string();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        if names.is_empty() && !dir.is_empty() { Err(format!("asset folder `{dir}` does not exist")) } else { Ok(names) }
    }
}

fn assets(files: &[(&str, &[u8])]) -> Arc<dyn Host> {
    Arc::new(AssetHost(files.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect()))
}

fn json_response(body: &str) -> ScriptResponse {
    ScriptResponse {
        code: 200,
        status: "OK".into(),
        headers: vec![("Content-Type".into(), "application/json".into()), ("X-Multi".into(), "a".into()), ("x-multi".into(), "b".into())],
        body: body.as_bytes().to_vec(),
        response_time_ms: 12,
        size: body.len() as u64,
    }
}

fn post_with(response: ScriptResponse, host: Arc<dyn Host>, sources: &[ScriptSource], tests: Option<TestBlock>) -> Outcome {
    let mut ctx = context(ScriptPhase::PostResponse, false, Variables::default());
    ctx.response = Some(response);
    ctx.host = host;
    ctx.tests = tests;
    run(sources, ctx, &Limits::default(), never())
}

fn assertion(expr: &str, operator: AssertOperator, value: &str) -> Assertion {
    Assertion { enabled: true, expr: expr.into(), operator, value: value.into(), ..Assertion::default() }
}

const FACE_BODY: &str = r#"{"status_code":"FACE-16","n":2,"list":[1,2],"s":"abc","e":"","o":{}}"#;

fn run_assertions(list: Vec<Assertion>) -> Outcome {
    let block = TestBlock { origin: "request Login tests".into(), assertions: list, script: None };
    post_with(json_response(FACE_BODY), assets(&[]), &[], Some(block))
}

#[test]
fn assertions_cover_every_operator() {
    use AssertOperator::*;
    // (expr, operator, value, passes)
    let table: Vec<(&str, AssertOperator, &str, bool)> = vec![
        ("res.status", Eq, "200", true),
        ("res.status", In, "200, 201", true),
        ("res.body.status_code", Eq, "FACE-16", true),
        ("res.body.n", Eq, "2", true),
        ("res.body.n", Eq, "3", false),
        ("res.body.n", Neq, "3", true),
        ("res.body.n", Neq, "2", false),
        ("res.body.n", Gt, "1", true),
        ("res.body.n", Gt, "2", false),
        ("res.body.n", Gte, "2", true),
        ("res.body.n", Gte, "3", false),
        ("res.body.n", Lt, "3", true),
        ("res.body.n", Lt, "2", false),
        ("res.body.n", Lte, "2", true),
        ("res.body.n", Lte, "1", false),
        ("res.status", In, "404, 500", false),
        ("res.status", NotIn, "404", true),
        ("res.status", NotIn, "200", false),
        ("res.body.s", Contains, "b", true),
        ("res.body.s", Contains, "z", false),
        ("res.body.list", Contains, "2", true),
        ("res.body.s", NotContains, "z", true),
        ("res.body.s", NotContains, "b", false),
        ("res.body.s", StartsWith, "ab", true),
        ("res.body.s", StartsWith, "bc", false),
        ("res.body.s", EndsWith, "bc", true),
        ("res.body.s", EndsWith, "ab", false),
        ("res.body.s", Matches, "^a.c$", true),
        ("res.body.s", Matches, "/^A/i", true),
        ("res.body.s", Matches, "^x", false),
        ("res.body.s", NotMatches, "^x", true),
        ("res.body.s", NotMatches, "^a", false),
        ("res.body.n", Between, "1,3", true),
        ("res.body.n", Between, "[3, 5]", false),
        ("res.body.list", Length, "2", true),
        ("res.body.list", Length, "3", false),
        ("res.body.e", IsEmpty, "", true),
        ("res.body.s", IsEmpty, "", false),
        ("res.body.s", IsNotEmpty, "", true),
        ("res.body.e", IsNotEmpty, "", false),
        ("null", IsNull, "", true),
        ("res.body.s", IsNull, "", false),
        ("res.body.nope", IsUndefined, "", true),
        ("res.body.s", IsUndefined, "", false),
        ("res.body.s", IsDefined, "", true),
        ("res.body.nope", IsDefined, "", false),
        ("res.body.s", IsTruthy, "", true),
        ("res.body.e", IsTruthy, "", false),
        ("res.body.e", IsFalsy, "", true),
        ("res.body.s", IsFalsy, "", false),
        ("res.body.n", IsNumber, "", true),
        ("res.body.s", IsNumber, "", false),
        ("res.body.s", IsString, "", true),
        ("res.body.n", IsString, "", false),
        ("true", IsBoolean, "", true),
        ("res.body.n", IsBoolean, "", false),
        ("res.body.list", IsArray, "", true),
        ("res.body.o", IsArray, "", false),
        ("res.body.o", IsJson, "", true),
        ("res.body.s", IsJson, "", false),
    ];
    let covered: std::collections::BTreeSet<&str> = table.iter().map(|(_, op, _, _)| op.token()).collect();
    for op in AssertOperator::ALL {
        assert!(covered.contains(op.token()), "operator {} has no case", op.token());
    }
    let outcome = run_assertions(table.iter().map(|(e, op, v, _)| assertion(e, *op, v)).collect());
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(outcome.tests.len(), table.len());
    for (result, (expr, op, value, passes)) in outcome.tests.iter().zip(&table) {
        assert_eq!(result.kind, TestKind::Assertion);
        assert_eq!(result.phase, ScriptPhase::PostResponse);
        let expected = if *passes { TestStatus::Passed } else { TestStatus::Failed };
        assert_eq!(result.status, expected, "{expr} {} {value}: {:?}", op.token(), result.message);
    }
}

#[test]
fn assertion_names_and_messages() {
    let outcome = run_assertions(vec![
        assertion("res.status", AssertOperator::Eq, "404"),
        assertion("res.body.s", AssertOperator::IsNumber, ""),
        assertion("res.body.s", AssertOperator::Gt, "1"),
        Assertion { enabled: false, ..assertion("res.status", AssertOperator::Eq, "1") },
    ]);
    assert_eq!(outcome.tests.len(), 3, "disabled rows are skipped");
    assert_eq!(outcome.tests[0].name, "res.status eq 404");
    assert_eq!(outcome.tests[0].message.as_deref(), Some("expected res.status equals 404, got 200"));
    assert_eq!(outcome.tests[1].name, "res.body.s isNumber");
    assert_eq!(outcome.tests[1].message.as_deref(), Some("expected res.body.s is number, got \"abc\""));
    assert!(outcome.tests[2].message.as_deref().is_some_and(|m| m.contains("expected a number")));
}

#[test]
fn assertion_value_interpolates_variables() {
    let mut ctx = context(ScriptPhase::PostResponse, false, Variables { locals: map(&[("want", "200")]), ..Variables::default() });
    ctx.response = Some(json_response(FACE_BODY));
    ctx.tests = Some(TestBlock { origin: "t".into(), assertions: vec![assertion("res.status", AssertOperator::Eq, "{{want}}")], script: None });
    let outcome = run(&[], ctx, &Limits::default(), never());
    assert_eq!(outcome.tests[0].status, TestStatus::Passed, "{:?}", outcome.tests[0].message);
}

#[test]
fn assertion_expression_errors_fail_with_message() {
    let outcome = run_assertions(vec![
        assertion("res.body.o.x.y", AssertOperator::Eq, "1"),
        assertion("nosuchname", AssertOperator::IsDefined, ""),
        assertion("res.body.s", AssertOperator::Matches, "(unclosed"),
    ]);
    assert_eq!(outcome.tests.len(), 3);
    assert!(outcome.failures.is_empty());
    assert_eq!(outcome.tests[0].status, TestStatus::Failed);
    assert!(
        outcome.tests[0].message.as_deref().is_some_and(|m| m.starts_with("could not evaluate res.body.o.x.y: ")),
        "{:?}",
        outcome.tests[0].message
    );
    assert!(outcome.tests[1].message.as_deref().is_some_and(|m| m.starts_with("could not evaluate nosuchname: ")), "{:?}", outcome.tests[1].message);
    assert_eq!(outcome.tests[2].status, TestStatus::Failed);
}

#[test]
fn assertions_run_between_post_response_and_tests_script() {
    let block = TestBlock {
        origin: "request Login tests".into(),
        assertions: vec![assertion(r#"pg.getVar("k")"#, AssertOperator::Eq, "v")],
        script: Some(r#"pg.test("script sees " + pg.getVar("k"), () => {});"#.into()),
    };
    let outcome = post_with(json_response("{}"), assets(&[]), &script(r#"pg.setVar("k", "v"); pg.test("post", () => {});"#), Some(block));
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    let names: Vec<(&str, TestKind, TestStatus)> = outcome.tests.iter().map(|t| (t.name.as_str(), t.kind, t.status)).collect();
    assert_eq!(
        names,
        vec![
            ("post", TestKind::Test, TestStatus::Passed),
            (r#"pg.getVar("k") eq v"#, TestKind::Assertion, TestStatus::Passed),
            ("script sees v", TestKind::Test, TestStatus::Passed),
        ]
    );
}

#[test]
fn a_failing_tests_script_is_reported_under_the_tests_origin() {
    let block = TestBlock { origin: "request Login tests".into(), assertions: Vec::new(), script: Some("throw new Error('boom');".into()) };
    let outcome = post_with(json_response("{}"), assets(&[]), &[], Some(block));
    assert_eq!(outcome.failures.len(), 1);
    assert_eq!(outcome.failures[0].origin, "request Login tests");
    assert!(outcome.failures[0].message.contains("boom"));
}

#[test]
fn assertions_count_toward_the_test_limit() {
    let list: Vec<Assertion> = (0..5).map(|_| assertion("res.status", AssertOperator::Eq, "200")).collect();
    let block = TestBlock { origin: "t".into(), assertions: list, script: None };
    let mut ctx = context(ScriptPhase::PostResponse, false, Variables::default());
    ctx.response = Some(json_response("{}"));
    ctx.tests = Some(block);
    let outcome = run(&[], ctx, &Limits { max_tests: 3, ..Limits::default() }, never());
    assert_eq!(outcome.tests.len(), 3);
    assert_eq!(outcome.failures.len(), 1);
}

#[test]
fn pg_has_every_pm_member_and_bruno_methods() {
    let outcome = post(
        r#"
        const missing = Object.keys(pm).filter((k) => !(k in pg));
        const same = pg.test === pm.test && pg.expect === pm.expect && pg.sendRequest === pm.sendRequest && pg.variables === pm.variables;
        const pgNames = ["getVar","setVar","hasVar","deleteVar","getEnvVar","setEnvVar","hasEnvVar","deleteEnvVar","getGlobalVar","setGlobalVar",
          "getCollectionVar","setCollectionVar","getFolderVar","getRequestVar","getEnvName","getProcessEnv","interpolate",
          "setNextRequest","skipRequest","stopExecution"];
        const bruNames = pgNames.filter((n) => n !== "skipRequest" && n !== "stopExecution").concat(["getGlobalEnvVar","setGlobalEnvVar","sendRequest","sleep","cwd","runRequest"]);
        const noPg = pgNames.filter((n) => typeof pg[n] !== "function");
        const noBru = bruNames.filter((n) => typeof bru[n] !== "function");
        const reqNames = ["getUrl","setUrl","getMethod","setMethod","getName","getHeader","getHeaders","setHeader","setHeaders","deleteHeader","getBody","setBody"];
        const noReq = reqNames.filter((n) => typeof req[n] !== "function" || typeof pg.request[n] !== "function");
        const resNames = ["getStatus","getStatusText","getHeader","getHeaders","getBody","getResponseTime","getSize"];
        const noRes = resNames.filter((n) => typeof res[n] !== "function");
        const runner = ["setNextRequest","skipRequest","stopExecution"].filter((n) => typeof bru.runner[n] !== "function");
        pg.setVar("report", JSON.stringify({missing, same, noPg, noBru, noReq, noRes, runner, tt: test === pm.test, ee: expect === pm.expect,
          assets: ["base64","text","exists","list"].filter((n) => typeof pg.assets[n] !== "function"),
          rep: ["set","get","toObject"].filter((n) => typeof pg.report[n] !== "function")}));
        "#,
    );
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(
        outcome.variables.runtime["report"],
        r#"{"missing":[],"same":true,"noPg":[],"noBru":[],"noReq":[],"noRes":[],"runner":[],"tt":true,"ee":true,"assets":[],"rep":[]}"#
    );
}

#[test]
fn unsupported_bruno_methods_throw_actionable_errors() {
    for (call, name) in [("bru.sleep(10)", "bru.sleep"), ("bru.cwd()", "bru.cwd"), ("bru.runRequest('x')", "bru.runRequest")] {
        let outcome = post(call);
        assert_eq!(only_failure(&outcome), format!("{name} is not supported in Postguy scripts"));
    }
}

#[test]
fn bru_runtime_vars_round_trip() {
    let variables = Variables {
        runtime: map(&[("seed", "1"), ("shadowed", "runtime")]),
        data: vec![("shadowed".into(), serde_json::json!("data"))],
        locals: map(&[("local", "L")]),
        ..Variables::default()
    };
    let outcome = pre(
        r#"
        bru.setVar("a", "1");
        pg.setVar("b", {x: 1});
        const seen = [bru.getVar("seed"), pg.getVar("a"), bru.hasVar("a"), bru.hasVar("zzz"), pm.variables.get("shadowed"), pg.getRequestVar("local")];
        bru.deleteVar("seed");
        pg.setVar("seen", JSON.stringify(seen));
        pg.setVar("gone", String(bru.getVar("seed")));
        "#,
        variables,
    );
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    let runtime = &outcome.variables.runtime;
    assert_eq!(runtime["a"], "1");
    assert_eq!(runtime["b"], r#"{"x":1}"#);
    assert_eq!(runtime["seen"], r#"["1","1",true,false,"runtime","L"]"#);
    assert_eq!(runtime["gone"], "undefined");
    assert!(!runtime.contains_key("seed"));
    assert!(!outcome.variables.locals.contains_key("a"), "runtime writes never land in locals");
}

#[test]
fn bruno_env_global_and_collection_vars_use_the_pm_scopes() {
    let variables = Variables {
        environment: Some(("dev".into(), map(&[("host", "h")]))),
        globals: map(&[("g", "1")]),
        collection: map(&[("c", "2")]),
        folders: map(&[("f", "3")]),
        ..Variables::default()
    };
    let outcome = pre(
        r#"
        bru.setEnvVar("token", "t", {persist: true});
        bru.setGlobalVar("g2", "x");
        bru.setCollectionVar("c2", "y");
        pg.setVar("r", [bru.getEnvName(), bru.getEnvVar("host"), bru.getGlobalVar("g"), bru.getCollectionVar("c"), bru.getFolderVar("f"),
                       bru.getProcessEnv("host"), bru.interpolate("{{host}}-{{c}}")].join("|"));
        "#,
        variables,
    );
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(outcome.variables.runtime["r"], "dev|h|1|2|3|h|h-2");
    assert_eq!(outcome.variables.environment.as_ref().map(|e| e.1["token"].clone()), Some("t".to_string()));
    assert_eq!(outcome.variables.globals["g2"], "x");
    assert_eq!(outcome.variables.collection["c2"], "y");
    let none = pre(r#"bru.setEnvVar("a", "b");"#, Variables::default());
    assert!(only_failure(&none).contains("no environment is selected"));
}

#[test]
fn req_get_set_body_json() {
    let mut ctx = context(ScriptPhase::PreRequest, false, Variables::default());
    ctx.request.body = ScriptBody::Raw(r#"{"subject_id":"{{subject_id}}","n":1}"#.into());
    ctx.request.method = "POST".into();
    let sources = script(
        r#"
        req.setBody({...req.getBody(), image: "x"});
        req.setHeader("X-A", "1");
        req.setHeaders({"X-B": "2", "accept": "application/json"});
        req.deleteHeader("X-A");
        req.setUrl(req.getUrl() + "&z=1");
        req.setMethod("put");
        pg.setVar("h", JSON.stringify([req.getHeader("x-b"), req.getHeaders()["Accept"] || req.getHeaders()["accept"], req.method, req.url]));
        "#,
    );
    let outcome = run(&sources, ctx, &Limits::default(), never());
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    let ScriptBody::Raw(text) = &outcome.request.body else { panic!("body is {:?}", outcome.request.body) };
    let value: serde_json::Value = serde_json::from_str(text).expect("raw json");
    assert_eq!(value, serde_json::json!({"subject_id": "{{subject_id}}", "n": 1, "image": "x"}));
    assert_eq!(outcome.request.method, "PUT");
    assert_eq!(outcome.request.url, "https://api.test/users?a=1&z=1");
    assert!(outcome.request.headers.iter().all(|h| !h.key.eq_ignore_ascii_case("x-a")));
    assert!(outcome.request.headers.iter().any(|h| h.key == "X-B" && h.value == "2"));
    assert_eq!(outcome.variables.runtime["h"], r#"["2","application/json","PUT","https://api.test/users?a=1&z=1"]"#);
}

#[test]
fn req_setters_are_read_only_after_the_response() {
    let outcome = post(r#"req.setUrl("x");"#);
    assert_eq!(only_failure(&outcome), "req is read-only in post-response scripts");
    let outcome = post(r#"pg.request.setHeader("a", "b");"#);
    assert_eq!(only_failure(&outcome), "pg.request is read-only in post-response scripts");
}

#[test]
fn res_callable_query() {
    let body = r#"{"data":{"items":[{"id":1,"k":{"id":9}},{"id":2}]},"s":"x"}"#;
    let sources = script(
        r#"
        pg.setVar("out", JSON.stringify([
          res("data.items[1].id"), res("..id"), res("data.nope.x"), res.status, res.statusText, res.headers["content-type"],
          res.headers["x-multi"], res.getHeader("Content-Type"), res.getStatus(), res.getResponseTime(), res.body.s, res.getBody().s,
          res.getSize(), res.getHeaders()["content-type"]
        ]));
        "#,
    );
    let outcome = post_with(json_response(body), assets(&[]), &sources, None);
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    let header_bytes = ("Content-Type".len() + "application/json".len() + 4) + ("X-Multi".len() + 1 + 4) + ("x-multi".len() + 1 + 4);
    let expected = format!(
        r#"[2,[1,9,2],null,200,"OK","application/json","a, b","application/json",200,12,"x","x",{{"body":{},"headers":{header_bytes},"total":{}}},"application/json"]"#,
        body.len(),
        body.len() + header_bytes
    );
    assert_eq!(outcome.variables.runtime["out"], expected);
}

const UTILS_JS: &str = r#"
const fs = require("fs");
const path = require("path");
exports.img = () => fs.readFileSync(path.join(__dirname, "images/a.png")).toString("base64");
exports.dir = __dirname;
exports.file = __filename;
"#;

#[test]
fn require_loads_asset_modules_with_fs_and_path_shims() {
    let host = assets(&[
        ("lib/utils.js", UTILS_JS.as_bytes()),
        ("lib/images/a.png", &[1, 2, 3, 250]),
        ("data.json", br#"{"k":[1,2]}"#),
        ("note.txt", b"hello"),
    ]);
    let sources = script(
        r#"
        const u = require("./lib/utils");
        const again = require("./lib/utils.js") === u;
        const crypto = require("crypto");
        const id = crypto.randomUUID();
        const fs = require("fs");
        pg.setVar("r", JSON.stringify({
          img: u.img(), dir: u.dir, file: u.file, again,
          b64: Buffer.from("{}", "utf8").toString("base64"),
          hex: Buffer.from("6a6b", "hex").toString("utf8"),
          roundtrip: Buffer.from(Buffer.from("héllo")).toString("utf8"),
          isBuf: Buffer.isBuffer(fs.readFileSync("/note.txt")),
          text: fs.readFileSync("note.txt", "utf8"),
          text2: fs.readFileSync("note.txt", {encoding: "utf8"}),
          exists: [fs.existsSync("lib/utils.js"), fs.existsSync("lib"), fs.existsSync("nope.txt")],
          dirList: fs.readdirSync("/lib"),
          json: require("./data.json").k,
          assets: [pg.assets.exists("note.txt"), pg.assets.text("note.txt"), pg.assets.base64("lib/images/a.png"), pg.assets.list("lib")],
          uuid: /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id),
          uuidDiffers: id !== crypto.randomUUID(),
          nodePrefix: require("node:path").basename("/a/b.txt", ".txt"),
          bufferModule: typeof require("buffer").Buffer.from,
        }));
        "#,
    );
    let outcome = run(&sources, context_with_host(host), &Limits::default(), never());
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    let value: serde_json::Value = serde_json::from_str(&outcome.variables.runtime["r"]).expect("json");
    assert_eq!(
        value,
        serde_json::json!({
            "img": "AQID+g==", "dir": "/lib", "file": "/lib/utils.js", "again": true, "b64": "e30=", "hex": "jk", "roundtrip": "héllo",
            "isBuf": true, "text": "hello", "text2": "hello", "exists": [true, true, false], "dirList": ["images", "utils.js"],
            "json": [1, 2], "assets": [true, "hello", "AQID+g==", ["images", "utils.js"]], "uuid": true, "uuidDiffers": true,
            "nodePrefix": "b", "bufferModule": "function"
        })
    );
}

fn context_with_host(host: Arc<dyn Host>) -> Context {
    let mut ctx = context(ScriptPhase::PreRequest, false, Variables::default());
    ctx.host = host;
    ctx
}

#[test]
fn require_npm_package_is_actionable_error() {
    let outcome = post(r#"require("lodash");"#);
    assert_eq!(only_failure(&outcome), "require('lodash') is not available in Postguy scripts; npm packages are not supported");
    let outcome = post(r#"require("axios");"#);
    assert_eq!(only_failure(&outcome), "require('axios') is not available in Postguy scripts; npm packages are not supported; use pg.sendRequest");
}

#[test]
fn require_of_a_missing_module_names_it() {
    let outcome = run(&script(r#"require("./nope");"#), context_with_host(assets(&[])), &Limits::default(), never());
    assert!(only_failure(&outcome).starts_with("Cannot find module './nope'"));
}

#[test]
fn require_cycle_returns_partial_exports() {
    let host = assets(&[
        ("a.js", b"exports.a1 = 1; const b = require('./b'); exports.a2 = b.sawA1;"),
        ("b.js", b"const a = require('./a'); exports.sawA1 = a.a1; exports.partialHasA2 = 'a2' in a;"),
    ]);
    let outcome = run(
        &script(r#"const a = require("./a"); pg.setVar("r", JSON.stringify([a.a1, a.a2, require("./b").partialHasA2]));"#),
        context_with_host(host),
        &Limits::default(),
        never(),
    );
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(outcome.variables.runtime["r"], "[1,1,false]");
}

#[test]
fn a_module_that_throws_is_not_cached() {
    let host = assets(&[("bad.js", b"throw new Error('bad module');")]);
    let outcome = run(
        &script(r#"let n = 0; for (let i = 0; i < 2; i++) { try { require("./bad"); } catch (e) { n++; } } pg.setVar("n", String(n));"#),
        context_with_host(host),
        &Limits::default(),
        never(),
    );
    assert_eq!(outcome.variables.runtime["n"], "2");
}

#[test]
fn the_module_count_is_capped() {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..70 {
        files.push((format!("m{i}.js"), b"module.exports = 1;".to_vec()));
    }
    let host: Arc<dyn Host> = Arc::new(AssetHost(files.into_iter().collect()));
    let outcome = run(&script(r#"for (let i = 0; i < 70; i++) require("./m" + i);"#), context_with_host(host), &Limits::default(), never());
    assert!(only_failure(&outcome).contains("at most 64 modules"));
}

#[test]
fn fs_write_is_refused() {
    let outcome = post(r#"require("fs").writeFileSync("x", "y");"#);
    assert_eq!(only_failure(&outcome), "fs.writeFileSync is not available in Postguy scripts (scripts can only read collection assets)");
}

#[test]
fn fs_cannot_leave_the_assets_and_unknown_encodings_are_refused() {
    let host = assets(&[("a.txt", b"x")]);
    let outcome = run(&script(r#"require("fs").readFileSync("/../etc/passwd");"#), context_with_host(host.clone()), &Limits::default(), never());
    assert_eq!(only_failure(&outcome), "path /../etc/passwd leaves the collection assets");
    let outcome = run(&script(r#"require("fs").readFileSync("a.txt", "ucs2");"#), context_with_host(host), &Limits::default(), never());
    assert_eq!(only_failure(&outcome), "Buffer encoding 'ucs2' is not supported in Postguy scripts");
}

#[test]
fn assets_without_a_collection_are_an_actionable_error() {
    let outcome = post(r#"pg.assets.base64("a.png");"#);
    assert_eq!(only_failure(&outcome), "assets are only available for requests saved in a collection");
}

#[test]
fn pg_assets_text_refuses_binary() {
    let host = assets(&[("a.bin", &[0xff, 0xfe, 0x00])]);
    let outcome = run(&script(r#"pg.assets.text("a.bin");"#), context_with_host(host), &Limits::default(), never());
    assert_eq!(only_failure(&outcome), "asset a.bin is not UTF-8 text; use pg.assets.base64");
}

#[test]
fn pg_report_set_caps() {
    let outcome = post(
        r#"
        pg.report.set("a", 1);
        pg.report.set("b", "text");
        pg.report.set("c", {x: [1, 2]});
        pg.report.set("d", true);
        pg.report.set("gone", 1);
        pg.report.set("gone", undefined);
        pg.report.set("long", "é".repeat(40000));
        pg.report.set("a", 2);
        const errors = [];
        try { pg.report.set("k".repeat(129), 1); } catch (e) { errors.push(e.message); }
        for (let i = 0; i < 120; i++) {
          try { pg.report.set("f" + i, i); } catch (e) { if (errors.length < 2) errors.push(e.message); }
        }
        pg.setVar("errors", JSON.stringify(errors));
        pg.setVar("got", JSON.stringify([pg.report.get("a"), pg.report.get("nope"), Object.keys(pg.report.toObject()).length]));
        "#,
    );
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(outcome.variables.runtime["errors"], r#"["pg.report keys may be at most 128 characters","pg.report holds at most 100 fields"]"#);
    assert_eq!(outcome.variables.runtime["got"], "[2,null,100]");
    let keys: Vec<&String> = outcome.report.keys().collect();
    assert_eq!(&keys[..5], ["a", "b", "c", "d", "long"]);
    assert_eq!(outcome.report.len(), 100);
    assert_eq!(outcome.report["a"], serde_json::json!(2));
    assert_eq!(outcome.report["c"], serde_json::json!(r#"{"x":[1,2]}"#));
    let long = outcome.report["long"].as_str().expect("string");
    assert!(long.ends_with('…') && long.len() <= 32 * 1024 + '…'.len_utf8(), "len {}", long.len());
}
