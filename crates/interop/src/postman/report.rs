//! The import report ([`ImportReport`]), the per-mapping decision record ([`MapOutcome`]),
//! and the deep-equality normalizer the goldens use.

use serde::{Deserialize, Serialize};

/// What one mapping step decided. `warnings` aggregate every [`MapOutcome::Normalized`],
/// `dropped` every [`MapOutcome::Dropped`]; `Direct` steps are counted, not spelled out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MapOutcome {
    /// JSON-ish path of the mapped key, e.g. `item[0].request.body.options.raw.language`.
    #[serde(default)]
    pub path: String,
    /// The decision itself; the text is user-presentable.
    #[serde(default)]
    pub detail: String,
}

/// Everything one import did: counts, warnings, drops. Serde-friendly so the UI can persist
/// or log it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ImportReport {
    #[serde(default)]
    pub collections: usize,
    #[serde(default)]
    pub environments: usize,
    #[serde(default)]
    pub globals: usize,
    #[serde(default)]
    pub requests: usize,
    #[serde(default)]
    pub folders: usize,
    #[serde(default)]
    pub examples: usize,
    /// Repairs and required assumptions (e.g. "missing `enabled`, assumed true").
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Content that could not be represented and was not inventable (e.g. a gRPC item).
    #[serde(default)]
    pub dropped: Vec<String>,
}

impl ImportReport {
    /// Record a normalized (mapped-with-repair) decision.
    pub(crate) fn normal(&mut self, path: &str, detail: impl Into<String>) {
        if self.warnings.len() < MAX_ENTRIES {
            self.warnings.push(format!("{path}: {}", detail.into()));
        }
    }

    /// Record a dropped decision.
    pub(crate) fn drop(&mut self, path: &str, reason: impl Into<String>) {
        if self.dropped.len() < MAX_ENTRIES {
            self.dropped.push(format!("{path}: {}", reason.into()));
        }
    }
}

/// A hostile input cannot balloon the report: caps on both lists.
pub(crate) const MAX_ENTRIES: usize = 200;

/// The shape [`crate::postman::import_value`] sniffed; the caller keeps it alongside the
/// model value to know what to export with.
#[derive(Debug, Clone, PartialEq)]
pub enum Imported {
    /// A collection (v2.1 or v2.0); boxed because collections dwarf the variable files.
    Collection(Box<postguy_model::Collection>),
    /// An environment (`_postman_variable_scope: "environment"` or unmarked).
    Environment(postguy_model::Environment),
    /// Globals (`_postman_variable_scope: "globals"`).
    Globals(postguy_model::Environment),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_caps_stay_bounded() {
        let mut report = ImportReport::default();
        for _ in 0..(MAX_ENTRIES + 50) {
            report.drop("x", "y");
            report.normal("x", "y");
        }
        assert_eq!(report.dropped.len(), MAX_ENTRIES);
        assert_eq!(report.warnings.len(), MAX_ENTRIES);
    }
}
