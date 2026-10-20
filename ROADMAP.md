# Postguy roadmap

**Stage: M4.4 complete · REVIEW STOP 3 (v0.1) pending** · imports, snippets and export dialogs in; remaining gaps in `docs/gaps.md`

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

- **2026-10-20 (fix, Add Request):** the sidebar's collection Add buttons and the toolbar New
  menu passed the collection id as a folder parent, so request creation failed; collection-level
  actions now pass `None` and folder actions keep the folder id. New Request from the welcome
  screen, tab-strip menu or command palette in a fresh workspace now creates a "New Collection"
  container instead of only printing "Create a collection first". Regression tests cover both
  the emitted sidebar intent and the engine/app path; live control verified a fresh workspace
  creating and opening its first request.
- **2026-10-20 (fix, Send):** sends and import/export jobs ran on worker threads but the UI only
  polled them on the next input event, so a click on Send could leave the status at "Sending…"
  with no response until the mouse moved. The app now requests a repaint every 50 ms while a
  send/import/export is in flight (no repaint when idle). Live localhost GET through the control
  channel returned `200 · 2 ms · 1542 B`, appeared in the viewer and History. `request.send` /
  `request.cancel` are registered commands too.

- **2026-10-20 (M4.1-M4.3, core):** Import dialog (paste Postman JSON/env/globals/cURL, multiple
  files, folders, drop anywhere, async native pickers, report with Open, numeric name suffixes,
  explicit replace); engine/automation import auto-detection with guided refusals for URL,
  OpenAPI/HAR/YAML. All eight snippet languages share `postguy-snippets` with golden files,
  indent/boilerplate options and `snippet_generate`; Copy as cURL. Export dialogs and tools for
  Postman/native collections, environments, globals and the all-data zip (secret values omitted
  unless opted in), plus safe zip import of JSON documents (traversal refused, arbitrary entries
  skipped). Import/export/snippet/zip tests; live dialog screenshots added to `xtask
  screenshots`. Remaining M4 gap: response-body save picker still writes to `<workspace>/files`.
- **2026-10-20 (M4.4):** parity rows for imports, snippets and zip exports updated; release
  budget app 8.63 MiB / CLI 3.80 MiB; `xtask bench --check` green (tree 5.5 ms,
  10 MB Raw 9.8 ms); screenshots regenerated for import, export, Rust snippet, dark/light and
  environment. **REVIEW STOP 3 (v0.1) pending.**

- **2026-10-20 (M3.10 exit measurements):** `xtask bench` added: headless frame-time capture for a
  10 000-request tree and a 10 MB Raw body, `--check` enforces the budgets. Release numbers:
  tree 5.8 ms first / 10.2 ms filter, 10 MB Raw 10.3 ms first / 10.1 ms average. A 10 MB
  minified line had frozen the viewer in a single galley; `LargeTextView` now chunks physical
  lines at 4 KiB and caches Raw text per buffer, with UTF-8/line tests. Release binary 8.31 MiB;
  first control inspect ~176 ms; idle CPU 0.2%; empty-workspace RSS 100.7 MiB (over the 90 MiB
  budget). Recorded remaining metrics and the control synthetic-input limitation in
  docs/gaps.md.
- **2026-10-20 (M3.10 control, verified live):** launched the release app with `--control` and
  drove it through `postguy-cli ui`: inspect returned state, `set console=true` opened the
  panel, screenshot wrote a real 2560x1600 PNG. The server now wakes an idle app through
  `Context::request_repaint` (no repaint loop), replaces any stale token file without following
  symlinks, caps concurrent handlers at 16, and removes the file only while it still holds this
  launch's token.

- **2026-10-20 (M3.10, partial):** Shortcut table in Settings (fed by the command registry) and
  keyboard dispatch for Cmd+N/I/K/Comma/W/Shift+T and Alt variants. Opt-in `--control FILE`
  server: loopback only, fresh 128-bit token per launch in an owner-only (0600) file, never
  logged; inspect/command/set/screenshot are wired to the running UI and `postguy-cli ui`
  authenticates with the token (refuses group/world-readable token files, honours `--root` for
  screenshot output). Screenshots run through egui's viewport screenshot event and are encoded
  as PNG; `xtask screenshots` renders dark/light/environment review shots into
  `target/screenshots/`. Click/key/type return an actionable refusal (no supported synthetic
  input path), recorded in docs/gaps.md and parity. 3 CLI client tests + 2 app control tests.
- **2026-10-20 (M3.9):** Console panel (All/Requests/Logs/Warnings/Errors filters, search,
  timestamps, clear, engine ring buffer with masked secrets), cookies manager (search, flags,
  delete, clear-all, script domain allowlist) and settings tab (General/Editor/Themes/
  Shortcuts/Data/Certificates/Proxy/About) wired through engine settings; bottom bar toggles
  sidebar/console/cookies/globals/trash/runner/two-pane/right, and the right code-snippet panel
  generates cURL/fetch/requests from the active draft. Settings save persists theme/proxy;
  cookies actions hit the jar; trash restore and rename/move undo are covered by app tests.
- **2026-10-20 (M3.8):** Command registry/palette (fuzzy search, Cmd+K, document items, keyboard
  selection), trash view with restore, and tree undo/redo for renames and moves. Settings
  Shortcuts table reads the same registry. Palette tests + app undo/trash tests.
- **2026-10-20 (M3.7):** Collection and folder detail tabs: Overview (description
  edit/preview, counts), Authorization (no Inherit at collection level), Variables table
  (enabled/secret/search/add) and Scripts shell; autosave/debounce, rename, duplicate/export/
  delete menu and dirty close; folder detail edits the folder with no Runs tab. 4 UI tests + app
  folder/collection persistence tests.

- **2026-10-20 (M3.6, core):** Environment/Globals editor: colour dot, editable title, search,
  description toggle, enabled/secret flags with masked values and a reveal toggle, per-row
  delete, drag reorder, add-by-typing and autosave through the engine (debounced; secret values
  split by the store). Globals open from the bottom bar. The tab-strip selector gained search,
  No Environment, checks and Create new environment, plus a grouped Variables quick-look over
  the active environment, globals and collection, with masked secrets and click-to-copy. App
  tests cover autosave, duplicate/delete, globals persistence and selector creation; 7 editor
  tests. A follow-up added per-chip hover popovers in variable fields (scope, value with secret
  masking, red unresolved note) and metadata-driven autocomplete. Still missing: the Set as
  variable context action and popover editing/add-to-scope.
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
