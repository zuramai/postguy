# postguy-automation

- **Layer:** L7 (enforced by `cargo xtask layers`)

Headless JSON-Schema tool table over the engine, the CLI backend and an opt-in UI control channel.
An MCP server is not implemented yet.

- `Session` opens an `Engine` (workspace, runtime, jar, console, history) under one optional
  `--root`; every path argument resolves through it with the identical refusal.
- HTTP tools: `request_send`/`request_cancel`, `response_get`/`save`/`render_body`,
  `cookies_list`/`set`/`delete`/`clear`, `history_list`/`open`/`clear`,
  `console_get`/`clear`, `variables_resolve`. `request_send` releases the session lock while
  the send runs, so `request_cancel` can reach it from another call; responses stay
  addressable by history id (last 20).
- Imports/exports: `import_text`/`import_file` auto-detect Postman JSON or cURL and suffix
  name clashes unless `replace`; `import_zip` reads `.json` documents from an archive;
  `workspace_export_zip` omits `.postguy` and `.trash`. `export_collection`,
  `export_environment`/`export_globals` (secret values opt-in) and `snippet_generate` share the
  UI's engine implementations.
- Bodies echoed to an agent are capped at 1 MiB (plus `truncated` and the true length); the
  full body is written by `response_save`.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
