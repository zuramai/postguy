# postguy-engine

- **Layer:** L6 (enforced by `cargo xtask layers`)

Facade and session: workspace handle, workspace edits, the send pipeline, console log, history, runtime handle.

- `Engine` — the session façade: opens the workspace, settings and persisted cookie jar, owns one
  tokio runtime (two background workers), the console ring buffer and the jar mutex. `send`
  blocks for CLI/automation and wraps the pipeline in `catch_unwind`; `send_async` hands the
  same work to the runtime for the UI. Settings updates are persisted atomically.
- `pipeline` — the send pipeline: scope chain (globals < collection < folders <
  environment < data < runtime < locals), body shortcodes (expanded before `{{var}}`; `asset://` paths for
  form-data/binary file rows), pre-request scripts (`scripts` module; `Settings.disable_scripts` skips them), auth
  inheritance and resolution, URL building (path vars, enabled query merge, IDNA,
  encoding), auth application, body + automatic headers, cookies, send, jar persistence,
  response model, History and Console recording (secret values masked before either exists).
  Stored requests resolve through their folder/collection ancestors; inline requests are for
  automation and tests.
- `assets` — per-collection Assets under `<workspace>/assets/<collection id>/`: `assets`,
  `list_asset_names`, `read_asset` (100 MiB cap), `import_assets` (files or folders; per-file, count
  and total-size caps; clashes get ` (2)`), `delete_asset`, `rename_asset`. Paths are validated
  (no `..`, absolute, dotfiles or symlink escapes). `duplicate_collection` copies assets.
- `run` — `run_collection`: one collection load per run (`SendTarget::Snapshot`, reloaded only when a
  script changed variables), `skip_history` so runs write no history, rows with `started_ms`,
  `pg.report` fields and a failed-response excerpt.
- `import` — also `import_bruno(dir, options)`: validates `bruno.json`, walks the folder (20,000
  `.bru` files at most), imports the collection and environments (secrets go to the secrets store)
  and copies every other file into Assets, keeping relative paths.
- Runtime variables: `runtime_variables`/`clear_runtime_variables` per collection id (inline sends
  use `""`); they live for the engine session, survive across requests and runs, and are never
  saved.
- `console` — 5000 entries / 24 h ring buffer with masked messages and 1 MiB detail bodies.
- `edit` — workspace edits behind the sidebar: collection/environment CRUD, folder and
  request creation, rename, deep duplicate with fresh ids, move/copy with cycle refusal,
  examples, trash parking and history deletion. Saves go through the store's atomic writers.
- `runtime` — one runtime, owned here; frontends never touch tokio directly.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
