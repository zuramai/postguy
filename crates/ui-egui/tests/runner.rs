//! The Runner tab: configuration, the Run/Cancel button, streaming and filtered results.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_model::{ScriptPhase, TestResult, TestStatus};
use postguy_ui_egui::runner::{RunRequestView, RunResultView, RunnerAction, RunnerRequestRow, RunnerState, RunnerView, runner_panel};

struct State {
    view: RunnerView,
    runner: RunnerState,
    action: Option<RunnerAction>,
    last: Option<RunnerAction>,
}

fn row(id: &str, name: &str, runnable: bool) -> RunnerRequestRow {
    RunnerRequestRow { id: id.into(), name: name.into(), method: if runnable { "GET".into() } else { "gRPC".into() }, path: String::new(), runnable }
}

fn view() -> RunnerView {
    RunnerView {
        collections: vec![("c1".into(), "Payments".into())],
        environments: vec![("e1".into(), "dev".into())],
        requests: vec![row("r1", "list", true), row("r2", "create", true), row("r3", "stream", false)],
    }
}

fn harness(runner: RunnerState) -> Harness<'static, State> {
    Harness::new_ui_state(
        |ui, state: &mut State| {
            state.action = runner_panel(ui, &state.view, &mut state.runner);
            if state.action.is_some() {
                state.last = state.action.clone();
            }
        },
        State { view: view(), runner, action: None, last: None },
    )
}

fn test(name: &str, status: TestStatus, message: Option<&str>) -> TestResult {
    TestResult { name: name.into(), status, message: message.map(str::to_string), phase: ScriptPhase::PostResponse }
}

fn request(iteration: u32, name: &str, tests: Vec<TestResult>) -> RunRequestView {
    RunRequestView {
        iteration,
        name: name.into(),
        method: "GET".into(),
        url: "http://x/".into(),
        status: Some(200),
        duration_ms: 12,
        tests,
        error: None,
        skipped: None,
    }
}

#[test]
fn runner_panel_runs_selected_requests_and_filters_results() {
    let mut harness = harness(RunnerState { collection: Some("c1".into()), ..RunnerState::default() });
    harness.run();
    assert!(harness.query_by_label_contains("Choose a collection and press Run").is_some(), "the empty results hint");
    assert!(harness.query_by_label_contains("stream (gRPC/WebSocket: skipped)").is_some(), "non-runnable rows say why");
    harness.get_by_label("Run Payments").click();
    harness.run();
    assert_eq!(harness.state().last, Some(RunnerAction::Run));

    harness.state_mut().runner.result = Some(RunResultView {
        id: "run1".into(),
        collection: "Payments".into(),
        requests: vec![
            request(0, "list", vec![test("status ok", TestStatus::Passed, None)]),
            request(0, "create", vec![test("created", TestStatus::Failed, Some("expected 200 to equal 201"))]),
            request(1, "list", vec![test("status ok", TestStatus::Passed, None)]),
        ],
        passed: 2,
        failed: 1,
        skipped: 0,
        duration_ms: 40,
        stopped: None,
    });
    harness.run();
    assert!(harness.query_by_label("Iteration 1").is_some() && harness.query_by_label("Iteration 2").is_some());
    assert!(harness.query_by_label_contains("2 passed").is_some() && harness.query_by_label_contains("1 failed").is_some());
    harness.get_by_label_contains("Failed 1").click();
    harness.run();
    assert!(harness.query_by_label_contains("GET create - 200").is_some());
    assert!(harness.query_by_label_contains("GET list - 200").is_none(), "the filter hides passing requests");
    harness.get_by_label("JUnit").click();
    harness.run();
    assert_eq!(harness.state().last, Some(RunnerAction::Export("junit")));
}

#[test]
fn unticking_a_request_removes_it_from_the_run_and_none_disables_run() {
    let mut harness = harness(RunnerState { collection: Some("c1".into()), ..RunnerState::default() });
    harness.run();
    harness.get_by_label("GET list").click();
    harness.run();
    assert!(harness.state().runner.excluded.contains("r1"));
    harness.get_by_label("Deselect all").click();
    harness.run();
    assert_eq!(harness.state().runner.excluded.len(), 2, "the non-runnable row never counts");
    harness.state_mut().last = None;
    harness.get_by_label("Run Payments").click();
    harness.run();
    assert_eq!(harness.state().last, None, "Run is disabled with nothing selected");
    harness.get_by_label("Select all").click();
    harness.run();
    assert!(harness.state().runner.excluded.is_empty());
}

#[test]
fn the_run_button_is_disabled_without_a_collection_and_becomes_cancel_while_running() {
    let mut harness = harness(RunnerState::default());
    harness.run();
    harness.get_by_label("Run").click();
    harness.run();
    assert_eq!(harness.state().last, None);

    harness.state_mut().runner = RunnerState {
        collection: Some("c1".into()),
        running: true,
        progress: (1, 4),
        result: Some(RunResultView { collection: "Payments".into(), requests: vec![request(0, "list", vec![])], ..RunResultView::default() }),
        ..RunnerState::default()
    };
    harness.run();
    assert!(harness.query_by_label_contains("1/4 requests").is_some());
    harness.get_by_label("Cancel").click();
    harness.run();
    assert_eq!(harness.state().last, Some(RunnerAction::Cancel));
}

#[test]
fn data_file_controls_report_choose_clear_and_errors() {
    let mut harness = harness(RunnerState { collection: Some("c1".into()), ..RunnerState::default() });
    harness.run();
    harness.get_by_label("Choose…").click();
    harness.run();
    assert_eq!(harness.state().last, Some(RunnerAction::PickData));

    harness.state_mut().runner.data_path = Some("/tmp/rows.csv".into());
    harness.state_mut().runner.data_preview = Some((vec!["id".into()], vec![vec!["7".into()]]));
    harness.run();
    assert!(harness.query_by_label("rows.csv").is_some());
    assert!(harness.query_by_label("7").is_some(), "the preview shows the first rows");
    harness.get_by_label("Clear").click();
    harness.run();
    assert_eq!(harness.state().last, Some(RunnerAction::ClearData));

    harness.state_mut().runner.data_path = None;
    harness.state_mut().runner.data_preview = None;
    harness.state_mut().runner.data_error = Some("row 2: found record with 1 fields".into());
    harness.run();
    assert!(harness.query_by_label("row 2: found record with 1 fields").is_some());
}
