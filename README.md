# Postguy

Postguy is a clean-room, offline, native API client for HTTP, gRPC and WebSocket, written in Rust
with egui. It does what the essential parts of Postman do — collections, environments, requests,
import/export — and nothing online: no account, no sync, no telemetry. It runs natively on macOS,
Windows, Linux and FreeBSD.

See [PROMPT.md](PROMPT.md) for the product brief and [AGENTS.md](AGENTS.md) for the rules that
bind every contributor. [PLAN.md](PLAN.md) is the execution order.

## Status

Bootstrap (M0): workspace skeleton and tooling. No features yet; nothing here is usable end to
end. The roadmap lives in [ROADMAP.md](ROADMAP.md), the milestone table in
[docs/roadmap.md](docs/roadmap.md).

## Building

```
cargo build -p postguy            # desktop app
cargo build -p postguy-cli        # headless CLI (run, tools, import/export, mcp, ui)
cargo xtask ci                    # fmt, clippy -D warnings, tests, layers, assets, parity
```

Every user-facing feature is reachable headlessly through `postguy-cli` (`cargo run -p
postguy-cli -- run --script steps.json --root DIR`); the desktop UI is layered on the same engine.

## Licence

MIT OR Apache-2.0. Bundled third-party material is listed in [ATTRIBUTION.md](ATTRIBUTION.md)
and [NOTICE](NOTICE).
