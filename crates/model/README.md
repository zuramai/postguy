# postguy-model

- **Layer:** L0 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** skeleton, planned for M1.2

Plain data and serde for collections, folders, requests, environments and history; unknown fields preserved in extras; no I/O.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
