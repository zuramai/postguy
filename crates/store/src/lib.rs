//! Postguy store (L2): the workspace on disk and the config-dir files.
//!
//! Layout (one folder = one workspace):
//!
//! ```text
//! <workspace>/
//!   postguy.json               schema, collection order, environment order, active environment id
//!   collections/<id>.json      one file per collection
//!   environments/<id>.json
//!   globals.json
//!   protos/<name>/...
//!   files/                     the default working directory
//!   history.jsonl              append-only, rotated, capped
//!   .trash/                    soft-deleted items with deletedAt metadata
//!   .postguy/
//!     runs/<collection-id>/<run-id>.json   the newest 20 collection runs
//!     secrets.json             0600; values of variables marked secret
//!     lock                     workspace lock with stale detection
//! ```
//!
//! Rules enforced here: atomic writes with `.bak` (via `postguy-util`), corrupt files renamed
//! `.corrupt-<time>` so the app starts without them and can say so, saves never lose previous
//! data on a failed write, secrets never sit inside collection or environment files, history
//! caps itself at 5000 entries by rotation.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod change;
pub mod config_dir;
pub mod history;
pub mod lock;
pub mod paths;
pub mod runs;
pub mod secrets;
pub mod trash;
pub mod workspace_file;

pub use paths::StorePaths;
pub use workspace_file::{LoadOutcome, Workspace, WorkspaceMeta};
