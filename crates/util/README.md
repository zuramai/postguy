# postguy-util

- **Layer:** L0 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** skeleton, planned for M1.1

Small helpers: atomic path utilities, size-capped readers, depth-limited JSON walker, ring buffer, fractional ordering, time.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
