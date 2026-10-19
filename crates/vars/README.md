# postguy-vars

- **Layer:** L1 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** skeleton, planned for M1.4

Variable scopes and precedence, {{var}} resolver with depth limit and cycle guard, dynamic variables, secret masking; pure.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
