# postguy-vars

- **Layer:** L1 (checked by `cargo xtask layers`)

Variable scopes and precedence, {{var}} resolver with depth limit and cycle guard, dynamic variables, secret masking; pure.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
