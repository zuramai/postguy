# postguy-testkit

- **Layer:** L3 (enforced by `cargo xtask layers`)

Loopback test servers (HTTP, TLS, OAuth token, gRPC, WebSocket); test-only support, a dev-dependency, never shipped.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
