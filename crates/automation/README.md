# postguy-automation

- **Layer:** L7 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** tool table over the engine (M2.9); MCP and the UI control channel later

Headless JSON-Schema tool table over the engine, the CLI backend, opt-in MCP server, opt-in UI
control channel.

- `Session` opens an `Engine` (workspace, runtime, jar, console, history) under one optional
  `--root`; every path argument resolves through it with the identical refusal.
- HTTP tools (M2.9): `request_send`/`request_cancel`, `response_get`/`save`/`render_body`,
  `cookies_list`/`set`/`delete`/`clear`, `history_list`/`open`/`clear`,
  `console_get`/`clear`, `variables_resolve`. `request_send` releases the session lock while
  the send runs, so `request_cancel` can reach it from another call; responses stay
  addressable by history id (last 20).
- Bodies echoed to an agent are capped at 1 MiB (plus `truncated` and the true length); the
  full body is written by `response_save`.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
