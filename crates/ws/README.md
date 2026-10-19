# postguy-ws

- **Layer:** L3 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** skeleton, planned for M6.1

WebSocket client: handshake with custom headers, TLS, frames, ping/pong, reconnect policy, message log.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
