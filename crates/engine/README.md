# postguy-engine

- **Layer:** L6 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** send pipeline, history, console and runtime (M2.8); edits/undo and commands later

Facade and session: workspace handle, edits and undo, the send pipeline, command registry, console log, history, runtime handle.

- `Engine` — the session façade: opens the workspace, settings and persisted cookie jar, owns one
  tokio runtime (two background workers), the console ring buffer and the jar mutex. `send`
  blocks for CLI/automation and wraps the pipeline in `catch_unwind`; `send_async` hands the
  same work to the runtime for the UI. Settings updates are persisted atomically.
- `pipeline` — PROMPT.md 5.3.2 steps 1–12: scope chain (globals < collection < folders <
  environment), the explicit no-op script hooks (warned in the Console until M8), auth
  inheritance and resolution, URL building (path vars, enabled query merge, IDNA, Appendix D
  encoding), auth application, body + automatic headers, cookies, send, jar persistence,
  response model, History and Console recording (secret values masked before either exists).
  Stored requests resolve through their folder/collection ancestors; inline requests are for
  automation and tests.
- `console` — 5000 entries / 24 h ring buffer with masked messages and 1 MiB detail bodies.
- `edit` — workspace edits behind the sidebar (M3.2): collection/environment CRUD, folder and
  request creation, rename, deep duplicate with fresh ids, move/copy with cycle refusal,
  examples, trash parking and history deletion. Saves go through the store's atomic writers.
- `runtime` — one runtime, owned here; frontends never touch tokio directly.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
