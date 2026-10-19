//! Postguy util: small, dependency-light helpers shared by every layer (L0).
//!
//! Everything here treats its inputs as hostile (AGENTS.md 4): reads are size-capped, JSON is
//! nesting-capped and parse errors are clear, atomic writes leave the previous file intact on
//! failure, and no function panics on any input.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod atomic;
pub mod error;
pub mod json;
pub mod order;
pub mod readers;
pub mod ring;
pub mod time;

pub use error::{Error, Result};
pub use readers::{read_file_capped, read_string_capped};
pub use ring::Ring;
