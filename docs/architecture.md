# Architecture

## Layer table (enforced by `cargo xtask layers`)

| Layer | Crate | Role |
|---|---|---|
| L0 | postguy-model | Plain data and serde: ids, collections, folders, requests (HTTP/gRPC/WS), key-value rows, bodies, auth configs, settings, environments, examples, history. No I/O; unknown fields preserved in `extras`. |
| L0 | postguy-util | Size-capped readers, depth-limited JSON walker, ring buffer, fractional ordering, atomic file helpers, time. |
| L1 | postguy-vars | Variable scopes and precedence, `{{var}}` resolver (depth limit, cycle guard), dynamic variables, secret masking. Pure. |
| L1 | postguy-urlkit | URL parse/build, per-component percent-encoding, query/URL two-way sync, path variables, host matching. Pure. |
| L2 | postguy-auth | Pure signing and credential algorithms; a canonical request in, headers/params out; no sockets. |
| L2 | postguy-interop | Import/export (Postman v2.0/v2.1, environments, cURL, OpenAPI, HAR) with `ImportReport`; bounded sizes and nesting. |
| L2 | postguy-store | Workspace on disk: atomic writes with `.bak`, lock file, trash, history, session, settings, secrets split. |
| L3 | postguy-http | hyper-direct connector: connect/TLS/timing, redirects, compression, cookie jar, proxy, auth retries, streaming, cancellation. |
| L3 | postguy-grpc | protox compile, descriptor pool, reflection, dynamic invoke, examples, status/trailers. |
| L3 | postguy-ws | tokio-tungstenite WebSocket client: handshake, TLS, frames, reconnect policy, message log. |
| L3 | postguy-script | rquickjs (QuickJS) sandbox with the `pm` API, Chai-style `pm.expect` and hard wall-clock, memory, stack and cancel limits. The `pm` API is Rust closures; `Settings.disable_scripts` is the off switch. |
| L3 | postguy-snippets | Code generation (cURL, fetch, requests, Go, reqwest, PowerShell, axios, grpcurl). Pure. |
| L3 | postguy-testkit | Loopback servers for tests only; dev-dependency, never shipped. |
| L4 | postguy-runner | Collection runs: plans, iterations, CSV/JSON data files, `setNextRequest` sequencing, results model, CLI/JSON/JUnit/HTML reporters. Pure. |
| L6 | postguy-engine | Façade and session: workspace handle, edits/undo, send pipeline (12 steps), command registry, console, history, runtime. |
| L7 | postguy-platform | OS services: dirs, dialogs, reveal, open URL, clipboard, system proxy, system font fallback. |
| L7 | postguy-ui-egui | The egui shell (eframe + glow); widgets and screens. |
| L7 | postguy-automation | Headless tool table over the engine, CLI backend, opt-in MCP, opt-in UI control channel. |
| apps | postguy, postguy-cli | Desktop app (with the opt-in `--control` server in `apps/postguy/src/control.rs`); headless CLI (`run`, `tools`/`call`, `import`/`export`, `snippet`, `mcp`, `ui`). |

Tooling: `xtask` (build system, gates, fuzz, icons/logo, size) is outside the layer system.

## Rules that hold everywhere

- **Never crash** (AGENTS.md 4): `Result` everywhere, no `unwrap`/`panic` in non-test code,
  `unsafe_code = "forbid"`, checked arithmetic on input-sized values, bounded recursion and
  allocations, `catch_unwind` at import/script/command entry points, `panic = "unwind"`.
- **Layering:** a crate may depend only on lower layers; nothing below L7 reaches egui/winit/
  eframe/rfd (the layers gate walks the normal-dependency closure over `cargo metadata`).
- **Concurrency:** `engine` owns one tokio runtime on dedicated threads; UI
  receives events and repaints once per batch; every long operation takes a cancellation token
  and wraps panics into error rows.
- **Workspace on disk:** one folder; `postguy.json`, `collections/<id>.json`,
  `environments/`, `globals.json`, `protos/`, `files/`, `history.jsonl`, `.trash/`,
  `.postguy/{secrets,cookies,responses,lock}`; atomic writes, `.bak`, corrupt-file recovery.

## Decisions made with measurements

1. Renderer: eframe glow renderer (smaller, faster warm start than wgpu).
2. Response bodies: response bodies as bytes + off-thread line index + `show_rows` virtualisation.
3. HTTP stack: hyper-direct connector (timings, redirects, header order, cancellation, tonic
   shares the same hyper stack).
4. Scripting: rquickjs (QuickJS, MIT), approved by the owner for its memory limit, interrupt
   handler and stack limit; boa_engine exposes none of these and is not used. `rust-alloc` stays
   off because it turns `set_memory_limit` into a no-op. Windows x86 and FreeBSD build the
   bindings with `bindgen` (no pregenerated ones ship for those targets).
5. Icons and logo: icons and logo as build-time path data + painter strokes; no SVG renderer shipped.
