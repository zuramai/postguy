# postguy-runner

- **Layer:** L4 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** skeleton, planned for M9.1

Collection runs: ordering, iterations, delays, data files, stop-on-error, cookie policy, results model, reporters.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
