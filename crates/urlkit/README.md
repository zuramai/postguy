# postguy-urlkit

- **Layer:** L1 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** skeleton, planned for M1.3

URL parse and build, percent-encoding per component, query/URL two-way sync, path variables, host-pattern matching; pure.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
