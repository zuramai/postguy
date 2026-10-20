//! Postguy automation (L7, headless): the JSON tool table every frontend shares (AGENTS.md 3).
//! The engine façade arrives with M2.8; until then this crate speaks `store` + `interop`
//! directly through [`session`], and every tool stays callable from `postguy-cli tools|call|run
//! --script`. M2.8 retargets [`session`] at the engine, not the tools' external shape.
//!
//! Safety rules enforced here, not by callers:
//! - every path argument resolves through [`root::Root`]; a path outside the allowed directory
//!   is refused with the SAME message whether or not it exists (an agent cannot probe the disk);
//! - unknown arguments are rejected with the allowed set named;
//! - imports are size- and depth-capped inside `postguy-interop`;
//! - destructive tools are marked (`destructive: true`) and deletes go to the workspace trash.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod error;
pub mod root;
pub mod session;
pub mod tools;

pub use error::{AutomationError, Result};
pub use root::Root;
pub use tools::{Automation, ToolDef, tools};

pub use session::Session;
