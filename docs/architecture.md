# Architecture

**Status:** bootstrap (written at M0; revise as crates land, with a revision line each time).

## Layer table (enforced by `cargo xtask layers`; PROMPT.md 4.1)

| Layer | Crate | Role |
|---|---|---|
| L0 | postguy-model | Plain data and serde: ids, collections, folders, requests (HTTP/gRPC/WS), key-value rows, bodies, auth configs, settings, environments, examples, history. No I/O; unknown fields preserved in `extras`. |
| L0 | postguy-util | Size-capped readers, depth-limited JSON walker, ring buffer, fractional ordering, atomic file helpers, time. |
| L1 | postguy-vars | Variable scopes and precedence, `{{var}}` resolver (depth limit, cycle guard), dynamic variables, secret masking. Pure. |
| L1 | postguy-urlkit | URL parse/build, per-component percent-encoding, query/URL two-way sync, path variables, host matching. Pure. |
| L2 | postguy-auth | Pure signing and credential algorithms; a canonical request in, headers/params out; no sockets. |
| L2 | postguy-interop | Import/export (Postman v2.0/v2.1, environments, cURL, OpenAPI, HAR) with `ImportReport`; bounded sizes and nesting. |
| L2 | postguy-store | Workspace on disk: atomic writes with `.bak`, lock file, trash, history, session, settings, secrets split. |
| L3 | postguy-http | hyper-direct connector: connect/TLS/timing, redirects, compression, cookie jar, proxy, auth retries, streaming, cancellation (ADR 0003). |
| L3 | postguy-grpc | protox compile, descriptor pool, reflection, dynamic invoke, examples, status/trailers. |
| L3 | postguy-ws | tokio-tungstenite WebSocket client: handshake, TLS, frames, reconnect policy, message log. |
| L3 | postguy-script | boa_engine sandbox with the `pm` API and limits (ADR 0004; `scripting` feature). |
| L3 | postguy-snippets | Code generation (cURL, fetch, requests, Go, reqwest, PowerShell, axios, grpcurl). Pure. |
| L3 | postguy-testkit | Loopback servers for tests only; dev-dependency, never shipped. |
| L4 | postguy-runner | Collection runs: ordering, iterations, data files, stop-on-error, results model, reporters. |
| L6 | postguy-engine | Façade and session: workspace handle, edits/undo, send pipeline (12 steps), command registry, console, history, runtime. |
| L7 | postguy-platform | OS services: dirs, dialogs, reveal, open URL, clipboard, system proxy, system font fallback. |
| L7 | postguy-ui-egui | The egui shell (eframe + glow, ADR 0001); widgets and screens. |
| L7 | postguy-automation | Headless tool table over the engine, CLI backend, opt-in MCP, opt-in UI control channel. |
| apps | postguy, postguy-cli | Desktop app; headless CLI (`run`, `tools`/`call`, `import`/`export`, `snippet`, `mcp`, `ui`). |

Tooling: `xtask` (build system, gates, fuzz, icons/logo, size) is outside the layer system.

## Rules that hold everywhere

- **Never crash** (AGENTS.md 4): `Result` everywhere, no `unwrap`/`panic` in non-test code,
  `unsafe_code = "forbid"`, checked arithmetic on input-sized values, bounded recursion and
  allocations, `catch_unwind` at import/script/command entry points, `panic = "unwind"`.
- **Layering:** a crate may depend only on lower layers; nothing below L7 reaches egui/winit/
  eframe/rfd (the layers gate walks the normal-dependency closure over `cargo metadata`).
- **Concurrency** (PROMPT.md 4.3): `engine` owns one tokio runtime on dedicated threads; UI
  receives events and repaints once per batch; every long operation takes a cancellation token
  and wraps panics into error rows.
- **Workspace on disk** (PROMPT.md 4.4): one folder; `postguy.json`, `collections/<id>.json`,
  `environments/`, `globals.json`, `protos/`, `files/`, `history.jsonl`, `.trash/`,
  `.postguy/{secrets,cookies,responses,lock}`; atomic writes, `.bak`, corrupt-file recovery.

## Decisions made with measurements (ADR index in `plan/adr/`, local-only)

1. ADR 0001: eframe glow renderer (smaller, faster warm start than wgpu).
2. ADR 0002: response bodies as bytes + off-thread line index + `show_rows` virtualisation.
3. ADR 0003: hyper-direct connector (timings, redirects, header order, cancellation, tonic
   shares the same hyper stack).
4. ADR 0004: boa_engine scripting (loop/recursion/stack limits; wall-clock and memory gaps
   escalated to the owner) — rquickjs recorded as the approvable alternative.
5. ADR 0005: icons and logo as build-time path data + painter strokes; no SVG renderer shipped.
