# postguy-urlkit

- **Layer:** L1 (checked by `cargo xtask layers`)

URL parse and build, percent-encoding per component, query/URL two-way sync, path variables, host-pattern matching; pure.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
