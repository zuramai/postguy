# postguy-engine

- **Layer:** L6 (enforced by `cargo xtask layers`)

Facade and session: workspace handle, workspace edits, the send pipeline, console log, history, runtime handle.

- `Engine` — the session façade: opens the workspace, settings and persisted cookie jar, owns one
  tokio runtime (two background workers), the console ring buffer and the jar mutex. `send`
  blocks for CLI/automation and wraps the pipeline in `catch_unwind`; `send_async` hands the
  same work to the runtime for the UI. Settings updates are persisted atomically.
- `pipeline` — the send pipeline: scope chain (globals < collection < folders <
  environment), pre-request scripts (`scripts` module; `Settings.disable_scripts` skips them), auth
  inheritance and resolution, URL building (path vars, enabled query merge, IDNA,
  encoding), auth application, body + automatic headers, cookies, send, jar persistence,
  response model, History and Console recording (secret values masked before either exists).
  Stored requests resolve through their folder/collection ancestors; inline requests are for
  automation and tests.
- `console` — 5000 entries / 24 h ring buffer with masked messages and 1 MiB detail bodies.
- `edit` — workspace edits behind the sidebar: collection/environment CRUD, folder and
  request creation, rename, deep duplicate with fresh ids, move/copy with cycle refusal,
  examples, trash parking and history deletion. Saves go through the store's atomic writers.
- `runtime` — one runtime, owned here; frontends never touch tokio directly.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
