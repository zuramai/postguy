//! Postguy collection runner core (L4, pure: no I/O, no network): run configuration, request
//! plans, CSV/JSON data files, `setNextRequest` sequencing, the result model and the CLI, JSON,
//! JUnit and HTML reporters. The engine drives the actual sends and fills the result in.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod config;
pub mod data;
pub mod error;
pub mod plan;
pub mod report;
pub mod result;
pub mod sequence;

pub use config::{MAX_DELAY_MS, MAX_ITERATIONS, RunConfig};
pub use data::{DATA_FILE_CAP, DataSet, MAX_COLUMNS, MAX_ROWS, parse_data};
pub use error::RunError;
pub use plan::{PlannedRequest, plan};
pub use result::{IterationResult, RequestRun, RunEvent, RunResult, RunSummary, Totals};
pub use sequence::{MAX_STEPS_PER_ITERATION, Sequencer};
