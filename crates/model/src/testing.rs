//! Results of user scripts: tests (`pm.test`), script failures and the "what next" directive.
//! Plain data shared by the script sandbox, the engine, the runner and the UIs.

use serde::{Deserialize, Serialize};

/// Which script ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScriptPhase {
    PreRequest,
    PostResponse,
}

impl ScriptPhase {
    /// "pre-request" or "post-response", for messages.
    pub fn label(self) -> &'static str {
        match self {
            ScriptPhase::PreRequest => "pre-request",
            ScriptPhase::PostResponse => "post-response",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
}

/// One `pm.test(...)` outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub name: String,
    pub status: TestStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub phase: ScriptPhase,
}

/// A script that threw, timed out or ran out of memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptFailure {
    pub phase: ScriptPhase,
    /// "collection <name>", "folder <name>" or "request <name>".
    pub origin: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// What a script asked the runner to do next. `Default` is `Unchanged`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "target")]
pub enum NextRequest {
    #[default]
    Unchanged,
    Stop,
    Named(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_results_round_trip_in_camel_case() {
        let result = TestResult { name: "t".into(), status: TestStatus::Failed, message: Some("boom".into()), phase: ScriptPhase::PostResponse };
        let json = serde_json::to_string(&result).unwrap();
        assert_eq!(json, r#"{"name":"t","status":"failed","message":"boom","phase":"postResponse"}"#);
        assert_eq!(serde_json::from_str::<TestResult>(&json).unwrap(), result);
    }

    #[test]
    fn next_request_default_is_unchanged() {
        assert_eq!(NextRequest::default(), NextRequest::Unchanged);
        let named = serde_json::to_string(&NextRequest::Named("x".into())).unwrap();
        assert_eq!(named, r#"{"kind":"named","target":"x"}"#);
    }
}
