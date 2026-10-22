# Postguy

Postguy is a clean-room, offline, native API client for HTTP, gRPC and WebSocket, written in Rust
with egui. It does what the essential parts of Postman do — collections, environments, requests,
import/export — and nothing online: no account, no sync, no telemetry. It runs natively on macOS,
Windows, Linux and FreeBSD.

See [AGENTS.md](AGENTS.md) for the rules that bind every contributor and
[docs/architecture.md](docs/architecture.md) for the crate layout.

## Status

Desktop HTTP client with collections, environments, variables, request builder, response viewer,
imports (Postman/cURL/zip documents), code snippets, export dialogs and the common auth types.
WebSocket and gRPC clients exist at a basic level; scripts and the collection runner are not
implemented yet. Feature-by-feature state is in `parity/postman-features.toml`; known gaps are in
[docs/gaps.md](docs/gaps.md).

## Building

```
cargo build -p postguy            # desktop app
cargo build -p postguy-cli        # headless CLI (run --script, tools/call, ui); import/export via call
cargo xtask ci                    # fmt, clippy -D warnings, tests, layers, assets, parity
```

Every user-facing feature is reachable headlessly through `postguy-cli` (`cargo run -p
postguy-cli -- run --script steps.json --root DIR`); the desktop UI is layered on the same engine.

## Licence

MIT OR Apache-2.0. Bundled third-party material is listed in [ATTRIBUTION.md](ATTRIBUTION.md)
and [NOTICE](NOTICE).
