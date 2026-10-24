use postguy_model::{Id, ScriptFailure, TestResult, TestStatus};
use serde::{Deserialize, Serialize};

/// One request's run inside one iteration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRun {
    pub request_id: Id,
    pub name: String,
    pub path: Vec<String>,
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub duration_ms: u64,
    pub size_bytes: u64,
    #[serde(default)]
    pub tests: Vec<TestResult>,
    #[serde(default)]
    pub failures: Vec<ScriptFailure>,
    /// The send failed (DNS, TLS, timeout, a pre-request script error...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Why the request did not run (non-HTTP protocol, `skipRequest()`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

impl RequestRun {
    /// A request error, a script failure or a failed test.
    pub fn failed(&self) -> bool {
        self.error.is_some() || !self.failures.is_empty() || self.tests.iter().any(|test| test.status == TestStatus::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IterationResult {
    pub index: u32,
    pub requests: Vec<RequestRun>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    /// Requests that ran (skipped ones are counted apart).
    pub requests: usize,
    pub skipped_requests: usize,
    /// Requests with an error, a script failure or a failed test.
    pub failed_requests: usize,
    pub tests_passed: usize,
    pub tests_failed: usize,
    pub tests_skipped: usize,
    pub script_failures: usize,
}

impl Totals {
    pub fn add(&mut self, run: &RequestRun) {
        if run.skipped.is_some() {
            self.skipped_requests = self.skipped_requests.saturating_add(1);
            return;
        }
        self.requests = self.requests.saturating_add(1);
        if run.failed() {
            self.failed_requests = self.failed_requests.saturating_add(1);
        }
        self.script_failures = self.script_failures.saturating_add(run.failures.len());
        for test in &run.tests {
            let slot = match test.status {
                TestStatus::Passed => &mut self.tests_passed,
                TestStatus::Failed => &mut self.tests_failed,
                TestStatus::Skipped => &mut self.tests_skipped,
            };
            *slot = slot.saturating_add(1);
        }
    }
}

/// A finished run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub id: String,
    pub collection_id: Id,
    pub collection_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_name: Option<String>,
    pub started_ms: i64,
    pub finished_ms: i64,
    pub iterations: Vec<IterationResult>,
    pub totals: Totals,
    /// Why the run ended early ("cancelled", "stopped after X failed (stop on error)").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
}

impl RunResult {
    /// No failed request, no failed test, no script failure and it ran to the end.
    pub fn passed(&self) -> bool {
        self.totals.failed_requests == 0 && self.totals.tests_failed == 0 && self.totals.script_failures == 0 && self.stopped.is_none()
    }

    pub fn duration_ms(&self) -> u64 {
        u64::try_from(self.finished_ms.saturating_sub(self.started_ms)).unwrap_or(0)
    }

    pub fn summary(&self) -> RunSummary {
        RunSummary {
            id: self.id.clone(),
            started_ms: self.started_ms,
            finished_ms: self.finished_ms,
            iterations: self.iterations.len(),
            totals: self.totals,
            passed: self.passed(),
        }
    }
}

/// The one-line view of a stored run (the Runs tab, `run_list`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub id: String,
    pub started_ms: i64,
    pub finished_ms: i64,
    pub iterations: usize,
    pub totals: Totals,
    pub passed: bool,
}

/// Progress the engine reports while a run executes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "event")]
pub enum RunEvent {
    Started { iterations: u32, requests_per_iteration: usize },
    Request { iteration: u32, run: Box<RequestRun> },
    IterationFinished { index: u32 },
    Finished,
}
