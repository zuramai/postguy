//! The egui shell: theme, widgets and all screens
//!
//! Status: skeleton (planned M3.1). See the crate README and PROMPT.md for the scope.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Icon path data generated from assets/icons by `cargo xtask icons` (regenerate, never edit).
/// Public from M3.1 on, when the widgets consume it; until then it is the compiled proof of
/// the icon pipeline.
pub mod icons_gen;
