use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use postguy_model::{NextRequest, ScriptPhase, TestStatus};
use postguy_script::{Context, Header, Host, Info, Limits, Outcome, ScriptBody, ScriptRequest, ScriptResponse, ScriptSource, Variables, run};

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
fn require_is_refused_with_a_message() {
    let outcome = post(r#"require("fs");"#);
    assert_eq!(only_failure(&outcome), "require('fs') is not available in Postguy scripts");
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
