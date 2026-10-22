# postguy-runner

- **Layer:** L4 (enforced by `cargo xtask layers`)
- **Status:** not implemented yet; the crate is an empty placeholder.

Collection runs: ordering, iterations, delays, data files, stop-on-error, cookie policy, results model, reporters.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
