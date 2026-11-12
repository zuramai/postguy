# postguy-store

- **Layer:** L2 (checked by `cargo xtask layers`)

Workspace on disk: load/save, atomic writes with .bak, lock file, trash, history, session and settings files, secrets split.

Workspace layout: `postguy.json`, `collections/`, `environments/`, `globals.json`, `protos/`, `files/`,
`assets/<collection id>/` (collection Assets; the id uses the collection-file id allowlist), `history.jsonl`,
`.trash/`, `.postguy/`. The history file is written by single sends only; collection runs skip it.

- `keychain` — `KeyStore` over the OS credential store (`OsKeyStore`, `keyring` crate: macOS
  Keychain, Windows Credential Manager, Secret Service via pure-Rust zbus on Linux/FreeBSD) and
  `MemoryKeyStore` for tests. AI Agent keys live under service `postguy`, account
  `ai-agent/<provider>`; secrets never appear in errors. Call it off the UI thread.
- `agent_chat` — `.postguy/agent-chat.json`: 8 MiB read cap, corrupt files quarantined, saves
  keep the newest 500 messages and drop whole turns from the oldest end.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
