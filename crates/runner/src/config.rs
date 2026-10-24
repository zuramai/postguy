use postguy_model::Id;
use serde::{Deserialize, Serialize};

use crate::error::RunError;

/// Most iterations one run may ask for.
pub const MAX_ITERATIONS: u32 = 10_000;
/// Longest pause between requests.
pub const MAX_DELAY_MS: u64 = 600_000;

/// What to run and how.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    pub collection: Id,
    /// Run only this folder (and its subfolders).
    #[serde(default)]
    pub folder: Option<Id>,
    #[serde(default)]
    pub environment: Option<Id>,
    /// `None`: one iteration per data row, or one.
    #[serde(default)]
    pub iterations: Option<u32>,
    #[serde(default)]
    pub delay_ms: u64,
    /// Stop after a request error, a script failure or a failed test.
    #[serde(default)]
    pub stop_on_error: bool,
    /// An ordered subset of request ids; `None` runs every request in tree order.
    #[serde(default)]
    pub requests: Option<Vec<Id>>,
}

impl RunConfig {
    /// The iteration count: the explicit value, else one per data row, else one.
    pub fn effective_iterations(&self, data_rows: Option<usize>) -> Result<u32, RunError> {
        if self.delay_ms > MAX_DELAY_MS {
            return Err(RunError::BadConfig(format!("delay must be at most {MAX_DELAY_MS} ms (10 minutes)")));
        }
        let wanted = match (self.iterations, data_rows) {
            (Some(count), _) => u64::from(count),
            (None, Some(rows)) => u64::try_from(rows).unwrap_or(u64::MAX),
            (None, None) => 1,
        };
        if wanted == 0 || wanted > u64::from(MAX_ITERATIONS) {
            return Err(RunError::BadConfig(format!("iterations must be between 1 and {MAX_ITERATIONS}")));
        }
        Ok(u32::try_from(wanted).unwrap_or(MAX_ITERATIONS))
    }
}
