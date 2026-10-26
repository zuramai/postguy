//! The one description of a run report's columns. CSV, XLSX and PDF are all rendered from a
//! [`ReportTable`], so they can never disagree about what a column means.

use postguy_model::{TestKind, TestStatus};
use postguy_util::time::{DateStyle, compact_duration, format_datetime};
use serde_json::Value;

use crate::result::{RequestRun, RunResult};

/// Column positions of the fixed request columns (custom report columns follow).
pub const COL_NUMBER: usize = 0;
pub const COL_FOLDER: usize = 2;
pub const COL_REQUEST: usize = 3;
pub const COL_METHOD: usize = 4;
pub const COL_STATUS_CODE: usize = 6;
pub const COL_RESULT: usize = 7;
pub const COL_TIME_MS: usize = 8;
pub const COL_ASSERTIONS_PASSED: usize = 10;
pub const COL_ASSERTIONS_FAILED: usize = 11;
pub const COL_TESTS_PASSED: usize = 12;
pub const COL_TESTS_FAILED: usize = 13;
pub const COL_FAILURE_DETAILS: usize = 15;
pub const COL_EXCERPT: usize = 16;
/// Index of the first custom report column in a request row.
pub const FIRST_CUSTOM_COL: usize = 17;

/// Index of the status column of a test row.
pub const TEST_COL_STATUS: usize = 6;
/// Index of the message column of a test row.
pub const TEST_COL_MESSAGE: usize = 7;

/// One spreadsheet cell.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(String),
    Int(i64),
    Float(f64),
    Empty,
}

impl Cell {
    /// The cell as plain text (numbers formatted, empty as `""`).
    pub fn text(&self) -> String {
        match self {
            Cell::Text(text) => text.clone(),
            Cell::Int(value) => value.to_string(),
            Cell::Float(value) if value.is_finite() => value.to_string(),
            Cell::Float(_) | Cell::Empty => String::new(),
        }
    }

    /// The integer value, or 0 for any other cell.
    pub fn int(&self) -> i64 {
        match self {
            Cell::Int(value) => *value,
            _ => 0,
        }
    }
}

/// How a row is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStatus {
    Pass,
    Fail,
    Error,
    Skipped,
}

/// A run flattened into labelled rows.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportTable {
    pub title: String,
    pub summary: Vec<(String, String)>,
    pub request_headers: Vec<String>,
    pub requests: Vec<(RowStatus, Vec<Cell>)>,
    pub test_headers: Vec<String>,
    pub tests: Vec<(RowStatus, Vec<Cell>)>,
}

fn row_status(run: &RequestRun) -> RowStatus {
    if run.skipped.is_some() {
        RowStatus::Skipped
    } else if run.error.is_some() {
        RowStatus::Error
    } else if run.failed() {
        RowStatus::Fail
    } else {
        RowStatus::Pass
    }
}

fn result_label(status: RowStatus) -> &'static str {
    match status {
        RowStatus::Pass => "PASS",
        RowStatus::Fail => "FAIL",
        RowStatus::Error => "ERROR",
        RowStatus::Skipped => "SKIPPED",
    }
}

fn to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn count(run: &RequestRun, kind: TestKind, status: TestStatus) -> i64 {
    to_i64(run.tests.iter().filter(|test| test.kind == kind && test.status == status).count())
}

fn failure_details(run: &RequestRun) -> String {
    let mut lines = Vec::new();
    for test in run.tests.iter().filter(|test| test.status == TestStatus::Failed) {
        lines.push(format!("[FAIL] {}: {}", test.name, test.message.as_deref().unwrap_or("failed")));
    }
    for failure in &run.failures {
        let line = failure.line.map(|line| format!(" line {line}")).unwrap_or_default();
        lines.push(format!("[SCRIPT] {} {}{line}: {}", failure.phase.label(), failure.origin, failure.message));
    }
    if let Some(error) = &run.error {
        lines.push(format!("[ERROR] {error}"));
    }
    if let Some(reason) = &run.skipped {
        lines.push(format!("[SKIPPED] {reason}"));
    }
    lines.join("\n")
}

fn value_cell(value: &Value) -> Cell {
    match value {
        Value::Null => Cell::Empty,
        Value::String(text) => Cell::Text(text.clone()),
        Value::Number(number) => match number.as_i64() {
            Some(int) => Cell::Int(int),
            None => number.as_f64().map(Cell::Float).unwrap_or_else(|| Cell::Text(number.to_string())),
        },
        other => Cell::Text(other.to_string()),
    }
}

fn when(ms: i64, offset_seconds: i32) -> String {
    format_datetime(ms, offset_seconds, DateStyle::Full, ms)
}

/// Flatten `result` into the report table. `offset_seconds` is the UTC offset used for every
/// datetime (callers pass `local_offset_seconds(result.started_ms)`).
pub fn table(result: &RunResult, offset_seconds: i32) -> ReportTable {
    let mut custom: Vec<String> = Vec::new();
    for iteration in &result.iterations {
        for run in &iteration.requests {
            for key in run.report.keys() {
                if !custom.iter().any(|known| known == key) {
                    custom.push(key.clone());
                }
            }
        }
    }
    let mut request_headers: Vec<String> = [
        "#",
        "Iteration",
        "Folder",
        "Request",
        "Method",
        "URL",
        "Status code",
        "Result",
        "Response time (ms)",
        "Size (bytes)",
        "Assertions passed",
        "Assertions failed",
        "Tests passed",
        "Tests failed",
        "Started at",
        "Failure details",
        "Response excerpt (failed only)",
    ]
    .iter()
    .map(|label| (*label).to_string())
    .collect();
    request_headers.extend(custom.iter().cloned());
    let test_headers: Vec<String> =
        ["#", "Iteration", "Folder", "Request", "Kind", "Name", "Status", "Message"].iter().map(|label| (*label).to_string()).collect();

    let mut requests = Vec::new();
    let mut tests = Vec::new();
    let (mut passed, mut failed, mut errors, mut skipped) = (0_i64, 0_i64, 0_i64, 0_i64);
    let (mut assert_pass, mut assert_total, mut test_pass, mut test_total) = (0_i64, 0_i64, 0_i64, 0_i64);
    let (mut time_sum, mut time_count) = (0_i64, 0_i64);
    let mut number = 0_i64;
    for iteration in &result.iterations {
        let iteration_no = i64::from(iteration.index).saturating_add(1);
        for run in &iteration.requests {
            number = number.saturating_add(1);
            let status = row_status(run);
            match status {
                RowStatus::Pass => passed = passed.saturating_add(1),
                RowStatus::Fail => failed = failed.saturating_add(1),
                RowStatus::Error => errors = errors.saturating_add(1),
                RowStatus::Skipped => skipped = skipped.saturating_add(1),
            }
            if status != RowStatus::Skipped {
                time_sum = time_sum.saturating_add(i64::try_from(run.duration_ms).unwrap_or(i64::MAX));
                time_count = time_count.saturating_add(1);
            }
            let folder = run.path.join(" / ");
            for test in &run.tests {
                let is_assertion = test.kind == TestKind::Assertion;
                let (total, ok) = if is_assertion { (&mut assert_total, &mut assert_pass) } else { (&mut test_total, &mut test_pass) };
                *total = total.saturating_add(1);
                if test.status == TestStatus::Passed {
                    *ok = ok.saturating_add(1);
                }
                let (row, label) = match test.status {
                    TestStatus::Passed => (RowStatus::Pass, "PASSED"),
                    TestStatus::Failed => (RowStatus::Fail, "FAILED"),
                    TestStatus::Skipped => (RowStatus::Skipped, "SKIPPED"),
                };
                tests.push((
                    row,
                    vec![
                        Cell::Int(number),
                        Cell::Int(iteration_no),
                        Cell::Text(folder.clone()),
                        Cell::Text(run.name.clone()),
                        Cell::Text(if is_assertion { "Assertion" } else { "Test" }.to_string()),
                        Cell::Text(test.name.clone()),
                        Cell::Text(label.to_string()),
                        test.message.clone().map(Cell::Text).unwrap_or(Cell::Empty),
                    ],
                ));
            }
            let mut cells = vec![
                Cell::Int(number),
                Cell::Int(iteration_no),
                Cell::Text(folder),
                Cell::Text(run.name.clone()),
                Cell::Text(run.method.clone()),
                Cell::Text(run.url.clone()),
                run.status.map(|code| Cell::Int(i64::from(code))).unwrap_or(Cell::Empty),
                Cell::Text(result_label(status).to_string()),
                Cell::Int(i64::try_from(run.duration_ms).unwrap_or(i64::MAX)),
                Cell::Int(i64::try_from(run.size_bytes).unwrap_or(i64::MAX)),
                Cell::Int(count(run, TestKind::Assertion, TestStatus::Passed)),
                Cell::Int(count(run, TestKind::Assertion, TestStatus::Failed)),
                Cell::Int(count(run, TestKind::Test, TestStatus::Passed)),
                Cell::Int(count(run, TestKind::Test, TestStatus::Failed)),
                if run.started_ms > 0 { Cell::Text(when(run.started_ms, offset_seconds)) } else { Cell::Empty },
            ];
            let details = failure_details(run);
            cells.push(if details.is_empty() { Cell::Empty } else { Cell::Text(details) });
            cells.push(run.response_excerpt.clone().map(Cell::Text).unwrap_or(Cell::Empty));
            for key in &custom {
                cells.push(run.report.get(key).map(value_cell).unwrap_or(Cell::Empty));
            }
            requests.push((status, cells));
        }
    }

    let ran = passed.saturating_add(failed).saturating_add(errors);
    let pass_rate = if ran == 0 { "n/a".to_string() } else { format!("{:.1}%", passed as f64 * 100.0 / ran as f64) };
    let average = if time_count == 0 { "n/a".to_string() } else { format!("{} ms", time_sum / time_count) };
    let summary: Vec<(String, String)> = vec![
        ("Collection".to_string(), result.collection_name.clone()),
        (
            "Folder".to_string(),
            match &result.folder {
                None => "All".to_string(),
                Some(id) => {
                    let first = result.iterations.iter().flat_map(|iteration| &iteration.requests).find(|run| !run.path.is_empty());
                    first.map(|run| run.path.join(" / ")).unwrap_or_else(|| id.0.clone())
                }
            },
        ),
        ("Environment".to_string(), result.environment_name.clone().unwrap_or_else(|| "None".to_string())),
        ("Started at".to_string(), when(result.started_ms, offset_seconds)),
        ("Finished at".to_string(), when(result.finished_ms, offset_seconds)),
        ("Duration".to_string(), compact_duration(i64::try_from(result.duration_ms()).unwrap_or(i64::MAX))),
        ("Iterations".to_string(), result.iterations.len().to_string()),
        ("Requests".to_string(), requests_len(&requests)),
        ("Passed".to_string(), passed.to_string()),
        ("Failed".to_string(), failed.to_string()),
        ("Errors".to_string(), errors.to_string()),
        ("Skipped".to_string(), skipped.to_string()),
        ("Pass rate".to_string(), pass_rate),
        ("Assertions".to_string(), format!("{assert_pass} / {assert_total}")),
        ("Tests".to_string(), format!("{test_pass} / {test_total}")),
        ("Average response time".to_string(), average),
        ("Stopped".to_string(), result.stopped.clone().unwrap_or_else(|| "No".to_string())),
        ("Generated by".to_string(), format!("Postguy {}", env!("CARGO_PKG_VERSION"))),
    ];
    ReportTable { title: format!("Postguy run report: {}", result.collection_name), summary, request_headers, requests, test_headers, tests }
}

fn requests_len(requests: &[(RowStatus, Vec<Cell>)]) -> String {
    requests.len().to_string()
}
