# postguy-automation

- **Layer:** L7 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** skeleton, planned for M1.8

Headless JSON-Schema tool table over the engine, the CLI backend, opt-in MCP server, opt-in UI control channel.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
