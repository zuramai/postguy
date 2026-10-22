# postguy-auth

- **Layer:** L2 (checked by `cargo xtask layers`)

Pure signing and credential algorithms (Basic, Bearer, Digest, Hawk, OAuth 1.0, AWS Sig v4, JWT); a canonical request in, headers/params out; no sockets.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
