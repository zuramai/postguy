# postguy-grpc

- **Layer:** L3 (enforced by `cargo xtask layers`)

gRPC: `.proto` compile (protox), descriptor pool, reflection client, dynamic unary and streaming invoke over tonic, example generator, status and trailers.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
