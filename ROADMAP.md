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

- **2026-10-20 (M3.6, core):** Environment/Globals editor: colour dot, editable title, search,
  description toggle, enabled/secret flags with masked values and a reveal toggle, per-row
  delete, drag reorder, add-by-typing and autosave through the engine (debounced; secret values
  split by the store). Globals open from the bottom bar. The tab-strip selector gained search,
  No Environment, checks and Create new environment, plus a grouped Variables quick-look over
  the active environment, globals and collection, with masked secrets and click-to-copy. App
  tests cover autosave, duplicate/delete, globals persistence and selector creation; 7 editor
  tests. Still missing: per-chip hover popovers and the Set as variable action.
- **2026-10-20 (M3.5, core):** Response viewer wired to the engine's cached send: status/time/
  size chips with phase, size and network popovers; Body/Cookies/Headers tabs; Pretty (JSON
  re-indent), Raw, Preview (text/binary message) and virtualised Hex; wrap toggle; search with a
  match count; Save Response to `<workspace>/files/` (also for Send and Download), Copy body/
  headers and Clear. Bodies use the precomputed-line LargeTextView; Pretty is capped at 5 MB.
  Sends land in the pane through a background thread, with cancellation and status messages.
  4 response UI tests + app send/download/save/clear tests. Still missing: syntax colours,
  image previews, search navigation/regex, off-thread indexing, file picker and streaming
  download.
- **2026-10-20 (M3.4, core):** Request builder: breadcrumb and editable name; method menu with
  REST colours; URL field with `{{var}}` chips, variable autocomplete and cURL paste detection
  (interop import in the app); Enter sends; split Send and Cancel with an elapsed timer; Save
  with dirty tracking; Params and `:pathVars` in two-way sync with the URL; authorization for
  Inherit/No Auth/Basic/Bearer/API key; header rows plus read-only automatic rows; all six body
  modes with per-mode drafts kept while switching (JSON Beautify); Scripts shells; per-request
  Settings; Docs. Sends run on a background thread against the unsaved draft through a new
  `SendTarget::StoredDraft` (ancestor scopes/auth inherited, nothing written by a send), and
  dirty tabs save through the engine before closing. 4 unit + 8 builder UI tests + 6 app tests.
  Header presets (built-in sets replacing in place), a show/hide toggle for the automatic rows
  and Params/Headers bulk edit landed next. Still missing: custom presets with a management
  dialog, the post-paste undo hint, and the response splitter (M3.5).
- **2026-10-20 (M3.3):** Tab strip: preview tabs replacing each other and promoting on a
  permanent open, painted close buttons and dirty dots, middle-click close, drag reorder,
  overflow search over open and recently closed tabs, the full tab context menu, the environment
  selector in the strip, and a strict Save/Don't save/Cancel dialog for dirty closes. Tabs
  serialise with drafts; the app writes tabs, sidebar sections/heights and theme into the store's
  session file atomically (throttled), restores them on launch, drops tabs whose documents no
  longer exist, and reports an unclean exit. 11 tab tests + 4 app session tests.
- **2026-10-20 (M3.2, core):** Sidebar tree and workspace editing: COLLECTIONS/ENVIRONMENTS/
  HISTORY with painted chevrons, 26 px rows, method labels, examples collapsed per request,
  preview-open clicks, arrows/Enter/F2/Delete, Cmd/Ctrl and Shift multi-select, inline rename,
  typed delete confirmation, drag/drop with insertion line plus folder nesting (Cmd/Ctrl copies),
  per-target context menus and the hover `...` menu, a name/URL filter with match highlighting
  and cached flattening (10 100 rows: 3.8 ms cold, 3.4 ms filtered in release), environment
  activation and history open/delete. Engine gained atomic collection/environment/tree edits
  (deep duplicate with fresh ids, move cycle refusal, trash parking). The app opens a workspace
  (`--workspace`) and applies sidebar intents. Still open: bulk multi-select operations, OS file
  drop, section splitters/persisted state (M3.3), P1 menu rows, and deferred Run/Export/Import
  targets from later milestones. Bulk delete/move/copy and draggable section splitters landed
  next, and pasted names get `Copy` suffixes.
- **2026-10-20 (M3.1):** Desktop shell and widget kit: eframe/glow app (min 900x560, default
  1280x800), semantic light/dark tokens with every text colour at >= 4.5:1 (tested) and
  System/Light/Dark via egui's OS signal, a from-scratch SVG path-data renderer for the
  generated Lucide icons (no SVG crate or icon font), custom square checkbox/radio/switch,
  grouped+searchable select, split button, context menu rows, modal, typed confirmation, inline
  rename, virtualised KeyValueTable, `{{var}}` field, CodeEditor, LargeTextView and Timeline.
  16 kittest widget tests plus 9 unit tests; headless `shot` example renders the real shell in
  both themes for review.
- **2026-10-20 (M2.9):** Automation tools over the engine: `request_send`/`request_cancel`
  (send outside the session lock so cancellation reaches in-flight work), response cache by
  history id with `response_get`/`save`/`render_body` (1 MiB echo cap), `cookies_*`,
  `history_*` (including atomic clear), `console_*` and `variables_resolve` with scope
  attribution. History appends are O(1) until the cap rotates, and the jar is rewritten only
  when a hop set cookies. M2 exit proved through the tools: redirect loop, gzip decode,
  self-signed TLS with the verify toggle, response cap and in-flight cancellation; the real CLI
  binary sends to the testkit via `run --script`. Send overhead (release, loopback, 100 KiB
  JSON): prepare < 1 ms, record ~4 ms, engine total ~5 ms vs bare client ~0.2 ms.
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
