//! Response-viewer tests: strip chips/tabs, Pretty/Raw/Hex, Headers and Cookies, the save/clear
//! menus and the size/time popovers.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_model::{ScriptFailure, ScriptPhase, TestResult, TestStatus};
use postguy_ui_egui::response::{ResponseAction, ResponseData, ResponseViewerState, response_view};

struct State {
    data: ResponseData,
    viewer: ResponseViewerState,
    action: Option<ResponseAction>,
    last: Option<ResponseAction>,
}

fn data() -> ResponseData {
    ResponseData {
        status: 200,
        status_text: "OK".into(),
        http_version: "HTTP/1.1".into(),
        headers: vec![("content-type".into(), "application/json".into()), ("set-cookie".into(), "sid=abc; Path=/; HttpOnly".into())],
        body: b"{\"b\":2,\"a\":1}".to_vec(),
        revision: 1,
        header_bytes: 120,
        body_encoded_bytes: 13,
        body_decoded_bytes: 13,
        content_encoding: None,
        connect_ms: 2,
        ttfb_ms: 10,
        total_ms: 14,
        download_ms: 4,
        redirects: 0,
        proxy: None,
        tests: Vec::new(),
        script_failures: Vec::new(),
    }
}

fn harness() -> Harness<'static, State> {
    Harness::new_ui_state(
        |ui, state: &mut State| {
            state.action = response_view(ui, &mut state.data, &mut state.viewer);
            if state.action.is_some() {
                state.last = state.action.clone();
            }
        },
        State { data: data(), viewer: ResponseViewerState::default(), action: None, last: None },
    )
}

#[test]
fn the_strip_shows_status_time_size_and_the_json_pretty_view() {
    let mut harness = harness();
    harness.run();
    assert!(harness.query_by_label_contains("200 OK").is_some());
    assert!(harness.query_by_label_contains("14 ms").is_some());
    assert!(harness.query_by_label_contains("13 B").is_some());
    assert!(harness.query_by_label_contains("\"a\": 1").is_some(), "Pretty orders and indents JSON");
}

#[test]
fn raw_hex_headers_and_cookies_are_reachable() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("Raw").click();
    harness.run();
    assert!(harness.query_by_label_contains("{\"b\":2,\"a\":1}").is_some());
    harness.get_by_label("Hex").click();
    harness.run();
    assert!(harness.query_by_label_contains("00000000").is_some());
    harness.get_by_label_contains("Headers 2").click();
    harness.run();
    assert!(harness.query_by_label_contains("content-type").is_some());
    harness.get_by_label_contains("Cookies 1").click();
    harness.run();
    assert!(harness.query_by_label_contains("sid").is_some());
    assert!(harness.query_by_label_contains("HttpOnly").is_some());
}

#[test]
fn the_menu_saves_and_clears_and_the_popovers_explain_the_numbers() {
    let mut harness = harness();
    harness.run();
    harness.get_by_label("...").click();
    harness.run();
    harness.get_by_label("Save body to file").click();
    harness.run();
    assert_eq!(harness.state().last, Some(ResponseAction::SaveBody));

    harness.get_by_label("...").click();
    harness.run();
    harness.get_by_label("Clear response").click();
    harness.run();
    assert_eq!(harness.state().last, Some(ResponseAction::Clear));

    harness.get_by_label_contains("14 ms").hover();
    harness.run();
    assert!(harness.query_by_label_contains("Download: 4").is_some());
    harness.get_by_label_contains("13 B").hover();
    harness.run();
    assert!(harness.query_by_label_contains("Headers: 120 B").is_some());
}

#[test]
fn search_counts_matches() {
    let mut harness = harness();
    harness.state_mut().viewer.search = "a".into();
    harness.run();
    assert!(harness.query_by_label_contains("match(es)").is_some(), "a match counter is visible");
}

#[test]
fn the_error_pane_shows_the_code_message_and_retry() {
    use postguy_ui_egui::response::{SendErrorView, error_view};

    #[derive(Default)]
    struct State {
        retry: bool,
    }
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut State| {
            if error_view(ui, &SendErrorView { message: "connecting to 127.0.0.1:9: Connection refused (os error 61)".into(), elapsed_ms: 3 }) {
                state.retry = true;
            }
        },
        State::default(),
    );
    harness.run();
    assert!(harness.query_by_label_contains("Could not send request").is_some());
    assert!(harness.query_by_label_contains("NETWORK").is_some());
    assert!(harness.query_by_label_contains("Connection refused").is_some());
    harness.get_by_label("Retry").click();
    harness.run();
    assert!(harness.state().retry);
}

#[test]
fn headers_render_as_a_left_aligned_table() {
    let mut harness = harness();
    let long = "x".repeat(600);
    harness.state_mut().data.headers.push(("x-long".into(), long.clone()));
    harness.set_size(egui::vec2(800.0, 600.0));
    harness.run();
    harness.get_by_label_contains("Headers 3").click();
    harness.run();
    let first_key = harness.get_by_label("content-type").rect();
    let second_key = harness.get_by_label("set-cookie").rect();
    let value = harness.get_by_label("application/json").rect();
    let long_value = harness.get_by_label(long.as_str()).rect();
    assert!((first_key.left() - second_key.left()).abs() < 0.5, "keys share a left edge: {first_key:?} vs {second_key:?}");
    assert!(first_key.right() < value.left(), "the value column starts right of the keys: {first_key:?} vs {value:?}");
    assert!((long_value.left() - value.left()).abs() < 0.5, "values share a left edge");
    assert!(long_value.right() <= 800.0, "a long value wraps inside the pane: {long_value:?}");
}

#[test]
fn a_long_line_does_not_widen_the_body() {
    let mut harness = harness();
    harness.state_mut().data.body = format!("{{\"k\":\"{}\"}}", "x".repeat(5000)).into_bytes();
    harness.state_mut().data.revision += 1;
    harness.set_size(egui::vec2(800.0, 600.0));
    harness.run();
    harness.run();
    let body = harness.get_by_label("Response body").rect();
    assert!(body.right() <= 800.0, "the body overflowed the pane: {body:?}");
    assert!(body.width() > 300.0, "the body should still fill the pane: {body:?}");
}

/// The text the last frame asked the platform to copy.
fn copied(harness: &Harness<'_, State>) -> Option<String> {
    harness.output().platform_output.commands.iter().find_map(|command| match command {
        egui::OutputCommand::CopyText(text) => Some(text.clone()),
        _ => None,
    })
}

fn harness_with_body(body: &str) -> Harness<'static, State> {
    let mut harness = harness();
    harness.state_mut().data.body = body.as_bytes().to_vec();
    harness.state_mut().data.revision += 1;
    harness.set_size(egui::vec2(800.0, 600.0));
    harness.run();
    harness
}

#[test]
fn cmd_d_selects_every_occurrence_and_copy_joins_them() {
    let mut harness = harness_with_body("{\"a\":\"x\",\"b\":\"x\"}");
    // Click the first row's text area (the caret lands at the end of the line), then walk back
    // onto the `x`.
    harness.get_by_label_contains("\"a\": \"x\"").click();
    harness.run();
    for _ in 0..3 {
        harness.key_press(egui::Key::ArrowLeft);
        harness.run();
    }
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::D);
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::D);
    harness.run();
    harness.event(egui::Event::Copy);
    harness.step();
    assert_eq!(copied(&harness).as_deref(), Some("x\nx"), "both occurrences are selected and joined");
}

#[test]
fn select_all_copies_the_whole_body() {
    let mut harness = harness_with_body("{\"a\":\"x\",\"b\":\"x\"}");
    harness.get_by_label_contains("\"a\": \"x\"").click();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.run();
    harness.event(egui::Event::Copy);
    harness.step();
    let pretty = copied(&harness).expect("select all then copy puts text on the clipboard");
    assert!(pretty.contains("\"a\": \"x\"") && pretty.contains("\"b\": \"x\""), "{pretty}");
    assert!(pretty.starts_with('{') && pretty.trim_end().ends_with('}'), "{pretty}");
}

fn test(name: &str, status: TestStatus, message: Option<&str>) -> TestResult {
    TestResult { name: name.into(), status, message: message.map(str::to_string), phase: ScriptPhase::PostResponse }
}

#[test]
fn test_results_tab_lists_and_filters_tests() {
    let mut harness = harness();
    harness.state_mut().data.tests = vec![
        test("status is 200", TestStatus::Passed, None),
        test("has an id", TestStatus::Passed, None),
        test("body is empty", TestStatus::Failed, Some("expected 3 to equal 0")),
    ];
    harness.run();
    harness.get_by_label_contains("Test Results (2/3)").click();
    harness.run();
    assert!(harness.query_by_label("status is 200").is_some());
    assert!(harness.query_by_label("has an id").is_some());
    assert!(harness.query_by_label("expected 3 to equal 0").is_some(), "a failed test shows its message");

    harness.get_by_label_contains("Failed 1").click();
    harness.run();
    assert!(harness.query_by_label("body is empty").is_some());
    assert!(harness.query_by_label("status is 200").is_none(), "the filter hides passed rows");
}

#[test]
fn test_results_explain_an_empty_list_and_show_script_failures() {
    let mut harness = harness();
    harness.state_mut().data.script_failures =
        vec![ScriptFailure { phase: ScriptPhase::PostResponse, origin: "request Login".into(), message: "boom".into(), line: Some(3) }];
    harness.run();
    harness.get_by_label_contains("Test Results").click();
    harness.run();
    assert!(harness.query_by_label("post-response script in request Login failed at line 3: boom").is_some());
    assert!(harness.query_by_label_contains("No tests ran").is_some());
}
