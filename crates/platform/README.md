# postguy-platform

- **Layer:** L7 (checked by `cargo xtask layers`)

OS services: config/data dirs, file dialogs, reveal in file manager, open URL, clipboard, system proxy, system font fallback.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
