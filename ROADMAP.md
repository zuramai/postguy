# Postguy roadmap

**Stage: bootstrap (M0)** · next: M0 exit review, then M1 (model, variables, storage, import/export)

Postguy is a clean-room, offline, native API client (HTTP, WebSocket, gRPC) in Rust + egui.
The full picture: `PROMPT.md` (product brief), `PLAN.md` (order of work, exit criteria),
`AGENTS.md` (rules). This file is the one-page summary and the progress log; the live state is
`plan/STATUS.md` (local-only).

## Milestone summary

| Tag | Milestones | Content |
|---|---|---|
| — | M0 | Workspace skeleton, xtooling, spikes with ADRs, tracking docs, logo |
| v0.1 | M1-M5 | Collections, environments, variables, HTTP, Postman/cURL import-export, desktop UI for those, remaining HTTP auth |
| v0.2 | M6-M7 | WebSocket, gRPC |
| v0.3 | M8-M9 | Scripts, tests, runner, CLI, OpenAPI/HAR import, MCP |
| v1.0 | M10 | Budget enforcement, accessibility, packaging, docs |

The owner reviews at each tag and at the end of M0 and M3.

## Progress log

- **2026-10-20 (M2.8):** Engine send pipeline: scope chain (globals/collection/folders/
  environment with secrets from the secrets store), explicit no-op script hooks, inherited
  P0 auth, URL building (path vars, query merge, IDNA, Appendix D encoding), body and automatic
  headers, cookie merging and persistence, proxy-aware send with redirects, the response model,
  masked History and Console recording, one owned tokio runtime, and `catch_unwind` around the
  pipeline so a panic becomes an actionable error. In-memory row defaults now match the file
  default (`enabled: true`). 10 engine integration tests + console/runtime unit tests.
- **2026-10-20 (M2.7):** Proxy and auth: `ProxyPolicy` resolves `HTTP_PROXY`/`HTTPS_PROXY`/
  `ALL_PROXY`/`NO_PROXY` or the custom Settings > Proxy block (custom wins), with wildcard
  bypass lists; the connector sends plain HTTP in absolute-form through an HTTP proxy, opens
  CONNECT tunnels for HTTPS, and speaks SOCKS5/SOCKS5h itself (RFC 1928/1929). Proxy basic auth
  rides plain requests and CONNECT; credentials are redacted from `Debug`. `http::auth` applies
  Basic, Bearer and API key (header or query), and errors actionably for v1-unsupported kinds.
  Testkit gained forwarding HTTP and SOCKS5 proxy fixtures. 6 wire + 5 unit tests.
- **2026-10-20 (M2.6):** Cookie jar (`http::jar`): RFC 6265 storage and matching, default
  path, expiration and Max-Age deletion, secure/HTTP-only flags, host-only vs Domain cookies,
  a minimal public-suffix guard (IP/dotless Domain refused), per-domain and total caps,
  RFC-ordered Cookie headers, a domain allowlist, and `send_with_cookies` capturing
  Set-Cookie on every hop with a per-request disable and manual-header merge. Persistence via
  `Workspace::load_cookies/save_cookies` (atomic, corrupt file quarantined). 9 unit + 5 wire
  tests.
- **2026-10-20 (M2.5):** Redirects: a manual loop (never hyper's) with per-hop records,
  RFC 3986 reference resolution for absolute/root/scheme-relative/path/query-only Locations,
  method rules (303 -> GET except HEAD; 301/302 POST -> GET unless follow-original; 307/308
  preserve), cross-host dropping of Authorization (toggleable), Cookie and body headers, the
  remove-referer setting, a hop cap and loop detection with actionable errors. Testkit gained
  a redirect-loop fixture and chain targets. 4 unit + 7 socket tests.
- **2026-10-20 (M2.4):** Response handling: automatic Content-Encoding decode for gzip,
  deflate (zlib with raw-deflate fallback), brotli and zstd (pure-Rust ruzstd), chained
  encodings in reverse order, decoded-size cap enforced WHILE reading plus a 200x ratio cap
  (compression-bomb regression tests), header/body encoded/decoded size breakdown for the
  hover chips, decode-off mode, and Send-and-Download streaming to a file with the same body
  cap and no full-body buffer (files hold wire bytes for now: documented follow-up).
  16 unit tests + 9 socket integration tests.
- **2026-10-20 (M2.3):** Request building: every body mode (form-data multipart with a
  collision-resistant boundary, urlencoded, raw with language content types, binary via the
  caller's file loader, GraphQL JSON with invalid-variables warning), the documented automatic
  header order (Host, User-Agent Postguy/version, Accept, Accept-Encoding gzip/deflate/br,
  Connection, calculated Content-Type/Content-Length, no-cache setting), user headers first and
  always winning (case-insensitive), CRLF/header-name injection refused, and no request-token
  header by design. 9 unit tests + a wire-order/multipart end-to-end send against the testkit.
  Known limit restated: HTTP/1 names go lowercase on the wire (http::HeaderName).
- **2026-10-20 (M2.2):** HTTP connector over hyper-direct (ADR 0003): custom timed connector
  (DNS→TCP→TLS phases recorded) with rustls native roots + optional user CA PEM, explicit
  insecure toggle, ALPN h1/h2, connect/whole-request timeouts, cancellation that aborts
  promptly, response body cap, and error classification that recovers the failing PHASE
  (DNS/TCP/TLS/request/download) from hyper-util's wrapper. One request per send (redirects
  M2.5, compression decode M2.4, cookies M2.6 are separate). Testkit gained a proper CA + leaf
  chain so verification is exercised with trust ON; 7 connector integration tests run against
  real sockets.
- **2026-10-20 (M2.1):** Testkit: loopback HTTP+HTTPS servers on ephemeral ports with the full
  fixture set (echo with ordered headers, redirects/chains, status codes, cookies, Basic/
  Bearer/Digest auth verified per RFC 7616, slow/streaming/compressed/huge bodies, OAuth token
  endpoint), real incremental chunk streaming, per-server self-signed certificates with a
  trustable PEM, and clean Drop shutdown. Hand-written pure-Rust zstd raw-block encoder for the
  zstd fixture (no C). 4 unit + 8 integration tests over real sockets; next is the M2.2
  hyper connector.
- **2026-10-20 (M1.8):** Automation: 33 tools over the workspace (workspace/collection/folder/
  request/env/globals/import/export families) with JSON schemas, unknown-argument rejection,
  root confinement with the identical refusal for existing and missing outside paths, and
  `postguy-cli tools|call|run --script`; secret variables now actually SPLIT into
  `.postguy/secrets.json` on every store save and rehydrate on load (env, globals, collection and
  folder scopes), imported Postman ids like `env-1` are accepted as file names (still
  traversal-proof), and deletion of items parks a JSON snapshot under `.trash/`. Four
  automation e2e tests + four CLI e2e tests; parity rows `f.agent-tools` and `f.run-script`
  now `partial` with cited evidence. M1 is complete; next is M2.1 testkit and the M2 HTTP engine.
- **2026-10-20 (M1.6, M1.9):** M1.6 Postman import/export landed with the vendored official
  v2.1.0/v2.0.0 schemas (Apache-2.0 test data, the AGENTS.md 1.1 closed-list pair, `postman_data`
  + attribution entries), typed auth mapping per Appendix C (jwt/asap read through verbatim),
  environment/globals scopes, `ImportReport`, and five fidelity goldens (typical v2.1, minimal
  v2.0, extras-at-every-level v2.1, env, globals) asserted twice for export stability. Gaps
  recorded: proxy/certificate blocks stay in extras (M5); unknown enum spellings degrade the
  auth block to Unknown verbatim; non-UTF-8 example bodies emit nothing. M1.9 fuzz targets for
  `url`, `vars`, `curl` wired through `postguy-cli fuzz-target` with attributed seed files; a
  45 s session ran ~360 executions with zero findings. `cargo xtask ci` green after both merges.
- **2026-10-19 (M1.4, M1.5, M1.7):** M1.4 vars: scope resolver with depth-10+cycle guards and
  spans for chips; 66 Appendix A dynamic variables with project-written word lists; secret
  masking helper; property tests for termination and idempotence over safe text. M1.5 store:
  the full workspace tree with atomic saves (`.bak`), corrupt-file quarantine (`.corrupt-<ns>`
  + recover-with-default), lock file with pid liveness + takeover, `.trash` with sidecar
  metadata + restore, rotating history (cap 5000), 0600 secrets split, change snapshots,
  config-dir settings/session with quarantine. M1.7 (agent-built, verified): cURL parse +
  generate in `interop` covering the whole PROMPT.md 5.7 flag table with quoting torture,
  warnings-not-errors, 22 unit + 3 property tests, clippy/fmt/layers clean. All gates green;
  `xtask ci` full sweep passes on `main` with everything merged.
- **2026-10-19 (M0.3-M0.5):** All five spikes measured with ADRs (`plan/adr/0001-0005`, local:
  glow renderer adopted; large-body strategy adopted; hyper-direct adopted; boa kept with the
  wall-clock/memory gap escalated to the owner at REVIEW STOP 1; icon/logo pipeline adopted).
  Icon pipeline committed: 6 Lucide icons pinned at 1.59.0, `xtask icons` generator + snapshot
  proof (relative-move per-element bug found and fixed with tests). Original logo mark
  committed with `xtask logo` raster sizes (16-512 px, byte-stable, attributed). M0.4: docs +
  parity checklist (111 features) written; STATUS/execution-plan updated. `cargo xtask ci`
  green on the full workspace.
- **2026-10-19 (M0):** Session start. Repo was empty (AGENTS.md, PROMPT.md, PLAN.md,
  `prompt_images/`); toolchain rustc/cargo 1.98 nightly, macOS arm64. `cargo-deny` and
  `cargo-bloat` are not installed, so per PLAN §0 the `xtask deny` and `xtask size` checks are
  implemented by xtask itself (deny.toml is still shipped for CI and for when the tools are
  approved). `prompt_images/` moved to `plan/postman/` as PLAN M0.1 specifies. Started M0.1
  (repo hygiene) and M0.2 (workspace skeleton).
