//! History entries: one row per user send, snapshots instead of references so
//! a History row can be reopened without the collection it came from.

use crate::ids::Id;
use crate::requests::RequestKind;
use serde::{Deserialize, Serialize};

/// One send, as the History sidebar and the tools' `history_*` need it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    #[serde(default)]
    pub id: Id,
    /// Unix millis (`postguy-util::time`).
    #[serde(default)]
    pub time_ms: i64,
    /// Snapshot of the request asSENT (resolved values).
    #[serde(default)]
    pub request: Option<RequestKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Round-trip duration as the response viewer shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Bytes of the response body (decoded size for compressed responses).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Reference into `.postguy/responses/` when "Save responses" is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_ref: Option<String>,
    #[serde(default, flatten)]
    pub extras: serde_json::Map<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sparse_history_entry_round_trips() {
        let raw = json!({"id": "h1", "timeMs": 1730000000000i64, "status": 200});
        let entry: HistoryEntry = serde_json::from_value(raw).unwrap();
        assert_eq!(entry.status, Some(200));
        assert_eq!(entry.duration_ms, None);
        let back = serde_json::to_value(&entry).unwrap();
        assert_eq!(back["timeMs"], json!(1730000000000i64));
        assert!(back.get("durationMs").is_none(), "default fields stay out of the file");
    }
}
