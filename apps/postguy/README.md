# postguy

The Postguy desktop app (eframe, glow backend). L7; it may use every crate, and reaches the
engine only through `postguy-engine` (checked by `cargo xtask layers`).
Run with `cargo run -p postguy` (`--workspace DIR` picks a workspace folder).

The opt-in UI control channel (AGENTS.md 3) starts only with `--control FILE`: it binds to
loopback, writes a fresh token in an owner-only file and serves `inspect`, `command`, `set` and
`screenshot` to `postguy-cli ui`. It is never enabled by default; `click`/`key`/`type` are
refused because the app does not emulate synthetic input.

Assets for the open collection load on a background thread (`postguy-assets`); import, rename and
delete run off the UI thread and refresh the cache. Dropped or picked folders that contain
`bruno.json` go to `Engine::import_bruno`. The Runner's "Download report" writes CSV, XLSX, PDF,
HTML, JSON or JUnit through `atomic_write`.
