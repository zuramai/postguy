# postguy-store

- **Layer:** L2 (checked by `cargo xtask layers`)

Workspace on disk: load/save, atomic writes with .bak, lock file, trash, history, session and settings files, secrets split.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
