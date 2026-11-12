# postguy-automation

- **Layer:** L7 (enforced by `cargo xtask layers`)

Headless JSON-Schema tool table over the engine, the CLI backend and an opt-in UI control channel.
An MCP server is not implemented yet.

- `Session` opens an `Engine` (workspace, runtime, jar, console, history) under one optional
  `--root`; every path argument resolves through it with the identical refusal.
  `Automation::with_engine` wraps an engine the caller already opened (the desktop app's).
- `agent` — the AI Agent loop (`Automation::agent_turn`): provider round trips through
  `Engine::ai_complete` (at most 25 per message, 16 tool calls each) over the `AGENT_TOOLS`
  subset of the table; destructive tools and export overwrites wait for `AgentHost::approve`;
  each call runs inside `catch_unwind`; results are capped at 64 KiB; every step is saved to the
  chat. Tools: `agent_chat` (headless turn, `allow_destructive`), `agent_chat_get`,
  `agent_chat_clear`, `agent_settings_get`/`set` (the key goes to the OS keychain, never
  returned). `collection_update`, `folder_get` and `folder_update` edit variables, auth,
  scripts and descriptions.
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
- Assets and Bruno: `asset_list`, `asset_import` (paths confined by the session root),
  `asset_delete` (destructive), `asset_rename`, `runtime_variables` (optionally `clear`) and
  `import_bruno` (a Bruno collection folder).
- `run_export` and `postguy-cli run --reporters` accept `json`, `junit`, `html`, `csv`, `xlsx`
  and `pdf`.
- Bodies echoed to an agent are capped at 1 MiB (plus `truncated` and the true length); the
  full body is written by `response_save`.
- Updates: `update_check` (latest release vs this build) and `update_download` (one asset into
  a folder inside `--root`, verified against the release's `SHA256SUMS.txt`). Installing over the
  desktop app stays with the app.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
