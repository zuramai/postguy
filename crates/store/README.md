# postguy-store

- **Layer:** L2 (checked by `cargo xtask layers`)

Workspace on disk: load/save, atomic writes with .bak, lock file, trash, history, session and settings files, secrets split.

Workspace layout: `postguy.json`, `collections/`, `environments/`, `globals.json`, `protos/`, `files/`,
`assets/<collection id>/` (collection Assets; the id uses the collection-file id allowlist), `history.jsonl`,
`.trash/`, `.postguy/`. The history file is written by single sends only; collection runs skip it.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
