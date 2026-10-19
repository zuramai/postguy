//! Postguy vars (L1, pure): `{{variable}}` scopes and resolution with depth and cycle guards,
//! dynamic variables (Appendix A, our own word lists) and the secret-mask helper.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod dynamic;
pub mod mask;
pub mod resolver;
pub mod words;

pub use mask::mask_secrets;
pub use resolver::{Resolution, Resolver, Scope, ScopeKind, VarHit, VarSource, resolve as resolve_text};

/// Max recursion depth of `{{var}}` chains (PROMPT.md 5.2: resolve recursively, depth 10).
pub const MAX_DEPTH: usize = 10;
