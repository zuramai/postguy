# postguy-engine

- **Layer:** L6 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** skeleton, planned for M2.8

Facade and session: workspace handle, edits and undo, the send pipeline, command registry, console log, history, runtime handle.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
