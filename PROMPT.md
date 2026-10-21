# Postguy: build prompt

Postguy is a clean-room, offline, native API client written in Rust with egui. It does what the essential parts of Postman do (collections, environments, HTTP / gRPC / WebSocket requests, scripts, runner, import/export) and nothing online. It must be small, start instantly, stay idle at ~0% CPU, and look clean and light.

This file is the product brief for agents and contributors. Read it with `AGENTS.md`, the single rules file (binding; wins any conflict). There is no `CLAUDE.md`. Where this file says "must", it is a requirement; "should" is a strong default that needs a written reason to depart from.

Table of contents

1. Principles and non-goals
2. Binding rules and how they apply here
3. Scope: tiers and version map
4. Architecture
5. Features
   - 5.1 Collections and folders
   - 5.2 Environments, globals and variables
   - 5.3 Requests: common model, HTTP, GraphQL, gRPC, WebSocket
   - 5.4 Response viewer
   - 5.5 Cookies, certificates, proxy, console, history, trash
   - 5.6 Automation: scripts, tests, runner, CLI, agent tools
   - 5.7 Imports and exports
   - 5.8 Code snippets
6. User interface
7. Settings
8. Performance and size budgets
9. Security and robustness
10. Testing and verification
11. Milestones
12. Docs, parity and tooling
13. Decisions already made (change only with a written reason)
14. Appendices

---

## 1. Principles and non-goals

### 1.1 Principles

1. **Offline and local.** No account, no sync, no telemetry, no update ping, no cloud anything. The app opens a folder on disk and works on it. The only network traffic is requests the user sends.
2. **Small and fast.** Budgets are in section 8 and are enforced in CI. Idle means no repaint loop. Big responses never freeze the UI.
3. **Clean and light.** Postman's information architecture (sidebar, tabs, request builder, response viewer) with fewer borders, no upsells, no banners, no onboarding popups, no illustrations. Hover reveals actions.
4. **Never crash, never lose work.** Every file, response, script and keystroke is untrusted input (AGENTS.md section 4). Unsaved drafts survive crashes and restarts.
5. **Headless first.** Every feature has an engine API, then a tool in `crates/automation`, then a UI. A feature is not done until the tool and an end-to-end test exist (AGENTS.md section 3).
6. **Compatible, not identical.** Import and export Postman Collection v2.1, environments and globals so people can move in and out. The native on-disk format may be richer.
7. **Rust only.** No handwritten JS/TS in the product or the build. Scripts written by users run in an embedded JS engine; the `pm` API itself is implemented in Rust.

### 1.2 Non-goals (do not build, do not stub, do not leave menu items for)

Sign-in, accounts, teams, workspaces as a cloud concept, sharing, forks, pull requests, comments, notifications, cloud sync, monitors, mock servers, Flows, API Builder/Specs/Spec Hub, published docs, API network, AI assistant (Postbot/agent mode), Vault cloud/shared vault, Newman-as-a-service, scheduled runs, performance tests, "upgrade" prompts, web (WASM) build, Socket.IO, MQTT, SOAP/WSDL, RAML, in-app browser, proxy capture/interceptor, Git integration, plugin system, cloud-linked "Request access" anything.

Later, not v1 (P3): Socket.IO, MQTT, SSE-specific UI beyond a basic event list, visualizer, native Git view, Insomnia/Hoppscotch import, directory-per-request storage, collection v3 YAML.

---

## 2. Binding rules and how they apply here

`AGENTS.md` is already written for Postguy and holds every rule that applies here: assets and clean-room (sections 1-2), agent control (3), never-crash (4) and working rules, quality gates, commits and how to run the app (5). It is derived from the sibling project `../pdfcraft`, so a few pdfcraft conventions carry over by design. Where this brief says "as pdfcraft does", it means the layout and tooling patterns in `../pdfcraft` (workspace shape, `xtask`, `automation` crate, `parity/` file); behaviour and code are never copied from Postman, and pdfcraft code is only reused as a pattern.

Postman-specific reading of the rules:

| Rule | In Postguy |
|---|---|
| Private observation notes | `plan/postman/` (git-ignored). The screenshots currently in `prompt_images/` move there. Never committed, published, linked, traced or used as a design source for artwork |
| Spec sources | Postman Collection Format v2.1 JSON schema (Apache-2.0, vendored as test data with an `ATTRIBUTION.toml` entry), public Postman docs, RFCs (HTTP, cookies, URI, WebSocket 6455, gRPC over HTTP/2, OAuth), the gRPC reflection proto, black-box observation of a real Postman install with synthetic data only |
| Other implementations | Do not port Newman, postman-sandbox, postman-runtime, postman-code-generators, Insomnia, Bruno or Hoppscotch. Behaviour comes from docs, schemas and observation |
| Fidelity | Unknown fields in imported collections, environments and settings are kept in an `extras` map and written back on export; every repair or drop is recorded in the import report |
| Control channel and MCP | `postguy-cli ui --control FILE` and `postguy-cli mcp`, both opt-in; `--root` confines every path |
| Web | No web/WASM target |

Assets (AGENTS.md section 1) apply in full. In practice for Postguy:

- **Icons:** Lucide (ISC) only. The ~40-60 icons used are listed in `ATTRIBUTION.toml`, one entry per icon file with SHA-256.
- **Fonts:** do not commit font files (AGENTS.md 1.4). v1 uses egui's built-in default fonts (they are Cargo-dependency assets: list them in `ATTRIBUTION.toml` as such). For characters those lack (CJK, emoji, Arabic...), read one installed system font at runtime as the last fallback, as AGENTS.md 1.4 describes. Never embed, ship or screenshot it. A nicer UI/mono font pair (for example Inter and JetBrains Mono, both OFL) needs the owner's decision on where fonts live and is out of scope until then.
- **Logo/app icon:** must be contributor-original (a simple geometric mark made for this project). Never trace or imitate Postman's.
- **Sample data:** every sample collection, proto, OpenAPI file and word list in the repo is written for this project. Test servers are local.
- **Screenshots** of Postguy for docs are produced by `cargo xtask screenshots` from our own UI only, with demo data.

Layering, lints and gates carry over: `unsafe_code = "forbid"`, the never-crash clippy denies on clean crates, `rustfmt max_width = 150`, `cargo xtask layers`, `cargo xtask assets`, scoped builds (`cargo clippy -p <crate> -j 2`), PRs into `main`, one task id per commit.

---

## 3. Scope: tiers and version map

Tiers: **P0** must work for v0.1. **P1** required for v1.0. **P2** nice to have, only after P0/P1 are solid. **P3** out of v1.

| Area | P0 | P1 | P2 |
|---|---|---|---|
| Collections | tree, nesting, rename, duplicate, delete, export, collection variables, collection-level auth, drag/drop, context menu | folder variables/auth/scripts, collection overview page, favourites, sort, multi-select, trash | pinned environment per collection |
| Environments | create/rename/duplicate/delete, variables table, active env, selector, globals, secret flag, import/export | quick-look popover, search/sort, colours | |
| HTTP | all methods, URL bar, params, path vars, headers, body (none, form-data, urlencoded, raw, binary), No Auth, Inherit, Basic, Bearer, API Key, response viewer (Pretty/Raw/Preview/Hex), cookies jar, redirects, TLS verify toggle, proxy, timings | Digest, JWT Bearer, OAuth 2.0 (auth code, PKCE, client credentials, password, implicit), OAuth 1.0, AWS Signature, Hawk, GraphQL body, cookies manager UI, client certificates, request Settings tab, response filter (JSONPath/XPath), examples (saved responses), Send and Download | NTLM, Akamai EdgeGrid, ASAP, HTTP/2 tuning (ping, windows), custom methods |
| gRPC | .proto import, invoke unary, TLS toggle, metadata, JSON message editor | server reflection, all 4 streaming modes, proto-aware autocomplete, saved messages, examples, settings, status/trailers | "Generate collection" from a service, grpcurl import/export |
| WebSocket | connect/disconnect, send text/JSON, message timeline | binary (base64/hex), saved messages, settings (timeout, reconnect, max size), headers/params, subprotocols, time-diff between two messages | |
| Automation | | scripts (pre-request, post-response/tests), `pm` API subset, test results, console, collection runner (GUI), CLI runner with reporters, data files, `postguy-cli run` tools, opt-in MCP | `setNextRequest` flows, `pm.sendRequest`, retry run |
| Imports | Postman v2.0/v2.1 collection, environment, globals, cURL (paste into URL bar + import dialog) | OpenAPI 3.x and Swagger 2.0 (JSON and YAML), HAR, .proto, drag-and-drop of files and folders | Postman data dump folder, Insomnia export |
| Exports | collection v2.1, environment, globals, native format | run results (JSON, JUnit, HTML), code snippets, all-data zip | |
| UI | window, sidebar, tabs, request builder, response viewer, light/dark/system, shortcuts, session restore, dirty tracking | command palette, console, right-side code snippet panel, two-pane layout, find in editors, settings tab, history | customisable shortcuts, UI zoom |

Version map (each is a tagged, shippable state):

- **v0.1** (M0-M5): collections, environments, variables, HTTP, Postman import/export, cURL import, full desktop UI for those.
- **v0.2** (M6-M7): WebSocket and gRPC.
- **v0.3** (M8-M9): scripts, tests, runner, CLI, OpenAPI/HAR import, remaining auth types, GraphQL.
- **v1.0** (M10): polish, performance budgets met, packaging for macOS, Windows, Linux and FreeBSD, docs.

---

## 4. Architecture

Same shape as pdfcraft: one Cargo workspace (`resolver = "3"`, edition 2024, `MIT OR Apache-2.0`), library crates in `crates/`, binaries in `apps/`, build tooling in `xtask/`. Rust version: the newest stable that egui requires; pin it in `rust-version`.

### 4.1 Crates and layers

`cargo xtask layers` enforces: a crate may depend only on lower layers; nothing below L7 depends on egui, winit, eframe or rfd.

| Layer | Crate | Role |
|---|---|---|
| L0 | `model` | Plain data and serde: ids, collections, folders, requests (HTTP, gRPC, WebSocket), key-value rows, bodies, auth configs, settings, environments, variables, examples, history entries. No I/O. Unknown JSON fields preserved in `extras`. |
| L0 | `util` | Small helpers: atomic path utilities, size-capped readers, depth-limited JSON walker, ring buffer, fractional ordering, time. |
| L1 | `vars` | Variable scopes, `{{var}}` resolver (recursive with depth limit and cycle guard), dynamic variables (`{{$guid}}`...), secret masking. Pure. |
| L1 | `urlkit` | URL parse, build, percent-encoding rules per component, query/URL two-way sync, path variables, host matching and wildcards. Pure. |
| L2 | `auth` | Pure signing/credential algorithms: Basic, Bearer, API key, Digest, Hawk, OAuth 1.0, AWS Sig v4, JWT (HS/RS/ES/PS), later NTLM/EdgeGrid/ASAP. Takes a canonical request description, returns headers/params. No sockets. OAuth 2.0 flow logic lives in `http` (needs I/O). |
| L2 | `interop` | Postman v2.0/v2.1 import/export, environment/globals files, native format, cURL parse and generate, OpenAPI/Swagger, HAR, grpcurl parse/generate. Returns an `ImportReport` (what was mapped, preserved, dropped). Bounded input sizes and nesting. |
| L2 | `store` | Workspace on disk (see 4.4): load/save, atomic writes with `.bak`, lock file, trash, history, session and settings files, secrets file, change watching. |
| L3 | `http` | HTTP client built on hyper (see 4.2): connect/TLS/timing, redirects, compression, cookie jar, proxy, client certs, auth application and retries (digest, OAuth 2.0 token handling), streaming bodies with size cap, cancellation. |
| L3 | `grpc` | `.proto` compile (protox), descriptor pool, reflection client, dynamic invoke (tonic with a dynamic codec), example message generator, proto-aware completion data, status and trailers. |
| L3 | `ws` | WebSocket client (tokio-tungstenite): handshake with custom headers, TLS, frames, ping/pong, reconnect policy, message log. |
| L3 | `script` | Embedded JS sandbox (boa_engine) with the `pm` API, `console`, `pm.test`/`pm.expect`, timeouts, memory/instruction limits, cancellation. Behind a cargo feature `scripting` (default on) so a lean build exists. |
| L3 | `snippets` | Code generation for cURL, HTTP, JS fetch, Python requests, Go net/http, Rust reqwest, PowerShell, Node axios, grpcurl. Pure. |
| L4 | `runner` | Collection/folder runs: ordering, iterations, delays, data files (CSV/JSON), flow control, stop-on-error, cookie policy, results model, reporters (cli, json, junit, html). |
| L6 | `engine` | Façade and session: workspace handle, open documents, edit/undo for tree operations, the send pipeline (see 5.3.2), command registry, console log, history, runtime handle. Frontends never reach past it. |
| L7 | `platform` | OS services: config/data dirs, file dialogs (rfd, UI side only), reveal in file manager, open URL, clipboard, system proxy discovery, system font fallback. |
| L7 | `ui-egui` | The egui shell: all widgets and screens. |
| L7 | `automation` | Headless JSON-Schema tool table over `engine`, the CLI backend, opt-in MCP server, opt-in UI control channel. |
| apps | `postguy` | Desktop app (eframe). |
| apps | `postguy-cli` | `run` (collection runner, Newman-like), `tools`, `call`/`run --script`, `mcp`, `ui` (drive running app), `import`, `export`, `snippet`. |

Crate and binary names carry the prefix `postguy-` (for example `postguy-model`, binary `postguy`). Each crate has a `README.md` (layer, status, API, conventions) like pdfcraft's `crates/automation/README.md`. Clean crates start with the never-crash `#![deny(...)]` list from AGENTS.md section 4.

### 4.2 Dependency choices (defaults; record departures in an ADR)

All dependency licences must pass `deny.toml` (permissive only: MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0, CC0, BSL-1.0, MIT-0; add CDLA-Permissive-2.0 only if a data crate needs it and the owner agrees). Always `default-features = false` and enable only what is used.

| Concern | Choice | Why |
|---|---|---|
| UI | `egui` + `eframe` (same major as pdfcraft at start) | Required |
| Renderer backend | `glow` first | Smaller binary and faster cold start than wgpu for a 2D text UI. M0 spike measures both (binary size, cold start to first frame, idle RSS on macOS and Linux) and records an ADR. wgpu is acceptable only if glow is unusable on a supported OS. Do not carry wgpu patches |
| Async | `tokio` (rt-multi-thread with 2 workers, net, time, sync, io-util, macros off) | One runtime on a background thread, owned by `engine` |
| HTTP | `hyper` 1.x + `hyper-util` + `hyper-rustls` + `rustls` (`ring` provider) with a hand-written connector | We must control redirects, cookie handling, per-phase timings (DNS, TCP, TLS, TTFB, download), header order and case, "follow original method", auth retries and TLS verification. `reqwest` hides most of that and we would bypass its redirect/cookie layers anyway. gRPC (tonic) already needs hyper, so one HTTP stack ships once. If this proves impractical, `reqwest` is the fallback; write down why |
| TLS roots | `rustls-native-certs` (OS trust store) plus optional user CA PEM | No bundled root list to attribute |
| Compression | `flate2` (miniz_oxide backend), `brotli`, `ruzstd` | Pure Rust only |
| Cookies | `cookie` + own jar (`http` crate), RFC 6265 rules | The jar must be visible, editable and scriptable |
| PKCS#12 client certs | `p12-keystore` or equivalent pure Rust | |
| Proxy | own CONNECT/SOCKS5 support in the connector (`tokio-socks` allowed); env vars `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY` | |
| gRPC | `tonic` (transport + generic `Grpc` client, no server, no codegen), `prost`, `prost-reflect` (+ `serde` feature for JSON mapping), `protox` for `.proto` parsing | No `protoc`, no build-time codegen |
| WebSocket | `tokio-tungstenite` with rustls | |
| JS engine | `boa_engine` | Pure Rust (fits "Rust only"), no C compiler, no foreign `unsafe`. Slower than QuickJS but fine for scripts. Set `RuntimeLimits` (loop iterations, recursion, stack) and a wall-clock timeout. `rquickjs` is smaller and faster but ships C and wraps FFI; use it only if boa fails the performance or compatibility bar, and ask the owner first |
| JSON | `serde_json` with `preserve_order`; custom token-level formatter for pretty-printing (no `Value` for huge bodies) | Key order and number text must survive |
| YAML (OpenAPI) | a maintained pure-Rust YAML crate | Cap input size, nesting and alias expansion (billion-laughs) |
| Time/rand/ids | `time` (minimal features), `getrandom`/`fastrand`, `uuid` | Avoid `chrono` and `rand` weight |
| Regex | `regex-lite` unless a feature needs full `regex` | |
| File watching | `notify` + debouncer (UI side), optional | |
| File dialogs | `rfd` (UI side only) | |
| Markdown (docs panes) | `pulldown-cmark` + own small egui renderer | |
| Icons | Lucide SVGs converted at build time by `xtask icons` into compact path data (kurbo `BezPath::from_svg`), drawn with egui's painter as strokes | No SVG renderer (resvg) in the release binary |
| Tests | `proptest`, `egui_kittest` | As pdfcraft |

Shared components from `craft-ui`/`craft-libs` (pdfcraft's git dependencies) are internal to another team. Do not depend on them. Build the few widgets needed here (square checkbox, outlined radio, etc.). Copy pdfcraft's `clippy.toml` `disallowed-methods` idea: egui's default checkbox/radio draw poorly on light themes, so use our own widgets in `ui-egui/src/widgets.rs`.

### 4.3 Concurrency model

- `engine` owns a tokio runtime on dedicated threads. The UI thread never blocks on I/O, parsing a large body, formatting, search or script execution.
- UI to engine: command messages. Engine to UI: events on a channel; the sender calls `egui::Context::request_repaint()` once per batch. A frame drains the channel with `try_recv`. Timers use `request_repaint_after`. Never call `request_repaint()` every frame.
- Every spawned task is wrapped so a panic becomes an error row (tokio reports `JoinError::is_panic`). Script and import entry points also keep a last-resort `catch_unwind` (AGENTS.md section 4). `panic = "unwind"` always.
- Every long operation takes a cancellation token: send, runner, invoke, connect, script, import, large-body formatting and search.
- Memory is bounded: response body cap (setting), message-log ring buffers, console ring buffer, history cap, per-request redirect cap.

### 4.4 Workspace on disk

A **workspace** is a folder. Default: the OS data dir (`.../Postguy/workspace`). The user may open any folder (File > Open Workspace, `--workspace DIR`). One instance holds a lock file per workspace; a stale lock is detected and reported, not trusted.

```
<workspace>/
  postguy.json                 # workspace meta: schema version, collection order, environment order, active env id
  collections/<id>.json        # one file per collection (folders and requests nested inside)
  environments/<id>.json
  globals.json
  protos/<name>/...            # imported .proto files and import paths for gRPC service definitions
  files/                       # default working directory for form-data / binary files
  history.jsonl                # append-only, rotated, capped (default 5000 entries)
  .trash/                      # soft-deleted items with deletedAt (default retention 30 days)
  .postguy/                    # local-only, safe to add to .gitignore
     secrets.json              # values of variables marked secret (0600 permissions)
     cookies.json              # cookie jar
     responses/                # cached large response bodies for history and examples
     lock
```

User config (not in the workspace), in the OS config dir: `settings.json`, `session.json` (open tabs, active tab, split sizes, sidebar widths, expanded tree nodes, unsaved drafts, window geometry), recent workspaces, client certificate list.

Rules: atomic writes (temp file in the same directory, `sync_all`, rename, keep one `.bak`); debounce autosave 300-800 ms and write only the changed file; a failed save keeps the in-memory state and tells the user; a corrupt file is renamed `.corrupt-<time>` and the app starts without it and says so; files changed on disk by another tool are detected (watcher plus re-scan on window focus): clean tabs reload silently, dirty tabs show a "changed on disk" bar with Reload / Keep mine. Files are pretty-printed JSON with stable key order so they diff well in git. The `store` crate hides this behind a trait so a directory-per-request layout can be added later.

Secret variable values never go into collection, environment or export files; they live in `.postguy/secrets.json` and are masked in the UI and console. (v1 does not encrypt at rest; say so in the docs. An OS keychain integration is P2.)

### 4.5 Data model (summary)

Native collection file (`"schema": "postguy/collection/1"`). IDs are random 128-bit ids (UUID text). Order is array order. Every struct carries `extras` for unknown fields.

```
Collection { id, name, description (markdown), variables[], auth?, scripts{prerequest?, test?}, pinned_environment?, favourite, items[] }
Item = Folder { id, name, description, variables[], auth?, scripts, items[] }
     | Request { id, name, description, kind: Http | Grpc | WebSocket, ..., examples[] }
KeyValue { key, value, description, enabled, kind: text|file (form-data), content_type?, secret? }
HttpRequest { method, url (raw string, may hold {{vars}} and :pathVars), params[] (derived from url, stored), path_vars[], headers[], body, auth, scripts, settings }
Body = None | FormData[] | UrlEncoded[] | Raw{language, text} | Binary{path} | GraphQL{query, variables}
Auth = Inherit | NoAuth | ApiKey | Basic | Bearer | Digest | Hawk | Jwt | OAuth1 | OAuth2 | AwsV4 | Ntlm | EdgeGrid | Asap   (each a typed struct)
GrpcRequest { url, tls, service_definition: Reflection | ProtoSet(name) | Inline files, method_path, message (JSON text), saved_messages[], metadata[], auth, scripts, settings }
WsRequest { url, params[], headers[], subprotocols[], saved_messages[] {name, format, text}, settings }
Example { id, name, original_request snapshot, status/code, headers, cookies, body(+encoding), language, time_ms }   // saved response
Environment { id, name, color?, variables[] {key, value, description, enabled, secret} }
HistoryEntry { id, time, request snapshot, status?, duration, size, response_ref? }
```

The URL string is the source of truth for the query; the Params table is a two-way view over it (unchecked rows are kept in the table but absent from the URL). Duplicate keys and empty values are preserved.

---

## 5. Features

### 5.1 Collections and folders

A collection is a project: it holds nestable folders and requests. (Original requirements, kept as written.)

- Collections can hold variables (key and value). Variables in a collection apply to all its requests and folders and their descendants.
- The user sees collections in the left side of the UI. Each collection row has a caret that toggles its contents. Clicking the collection name opens the collection detail in the main tab area.
- Right-clicking a collection shows a context menu: **Add request**, **Add folder**, **Run**, **Rename**, **Duplicate**, **Delete**, **Export collection**.

Required details beyond that:

**Tree rows.** Height ~30 px. Caret (collections, folders), folder/collection icon, name, truncated with ellipsis, full name in a tooltip. Requests show a short coloured method label (GET, POST, PUT, PATCH, DEL, HEAD, OPT; `gRPC`, `WS` for the other protocols) and the name. Examples (saved responses) appear nested under their request with a small icon, collapsed by default. Hovering a row reveals a `...` button (same menu as right-click) and, on collections, a `+` button (Add request) and a favourite star. Selected row has a subtle fill; the active tab's item is also marked in the tree ("Reveal in sidebar" does this).

**Clicks and keys.**
- Caret click toggles only. Name click opens the detail tab as a *preview tab* (italic title, replaced by the next preview). Double-click, editing or sending makes it permanent. A setting "always open in a new tab" turns preview off.
- Arrow keys move, Left/Right collapse/expand, Enter opens, F2 or Enter-on-selected renames inline, Delete deletes (with confirmation), Cmd/Ctrl+D duplicates, Cmd/Ctrl+C/X/V copy, cut, paste items (paste into a collection or folder; names get " Copy" suffix on clash), Cmd/Ctrl-click and Shift-click multi-select (bulk delete, move, copy).
- Drag and drop reorders and nests with a visible drop indicator (line between rows, outline on a folder target). Drop onto another collection moves. Cmd/Ctrl+drag copies. Dropping a file from the OS onto the sidebar opens the import dialog.

**Context menus** (right-click and `...`):

| Target | Items |
|---|---|
| Collection | Add request, Add folder, Run, Rename, Duplicate, Delete, Export collection. Also (P1): Add to favourites, Sort A-Z, Collapse all, Copy, Paste, Reveal in file manager |
| Folder | Add request, Add folder, Run, Rename, Duplicate, Delete, Copy, Cut, Paste |
| Request | Rename, Duplicate, Delete, Add example, Copy, Cut, Paste, Open in new tab, Copy as cURL |
| Example | Rename, Duplicate, Delete, Open as request |
| Environment | Set active, Rename, Duplicate, Export, Delete |

"Run" opens the Runner tab preconfigured for that collection or folder. "Duplicate" deep-copies with new ids. "Delete" moves to the trash; deleting a non-empty collection asks for confirmation (type the word `delete`). "Export collection" opens a file dialog and writes Postman v2.1 JSON (also offer "Postguy native").

**Add request** asks which protocol (HTTP, gRPC, WebSocket) via a small menu, creates "New Request" in place, opens it in a tab with the name field in edit mode.

**Sidebar search/filter** filters the tree live (name and URL, case-insensitive, debounced, runs in under one frame for 10 000 items), highlights matches and keeps parents of matches visible.

**Collection detail tab** (tabs: Overview, Authorization, Scripts, Variables, Runs):
- Header: icon, editable name (click to edit), `Run` button, a `...` menu (Rename, Duplicate, Export, Delete). Nothing about sharing, publishing or forks.
- **Overview:** Markdown description (edit/preview toggle), counts of folders and requests, created and last-updated time, "default environment" picker (pinned environment, P2).
- **Authorization:** the collection-level auth config (same auth editor as requests, without "Inherit"). A green dot on the tab label when an auth type is set.
- **Scripts:** Pre-request and Post-response editors (P1). Dot when non-empty.
- **Variables:** key/value table with enabled checkbox, description, secret flag, search, "Add variable" placeholder row. Dot when any exist.
- **Runs:** list of past runner runs for this collection (time, iterations, passed/failed, duration), open for results, delete.
Folder detail has the same tabs minus Runs and the default environment.

**Inheritance.** Auth: request, then each parent folder up to the collection; the first non-`Inherit` wins. Request `Inherit` shows the resolved parent type and an "Edit in parent" link. Scripts: collection, then folders outer to inner, then request, for both phases. Variables: see 5.2.

**Ordering.** Items keep array order. Runner order is tree order with each folder's contents before the root requests of the same level (document and test this explicitly, it is Postman's behaviour).

### 5.2 Environments, globals and variables

**Sidebar.** An ENVIRONMENTS section below COLLECTIONS (collapsible header, small uppercase label). Rows: environment name, a check badge on the active one. Click selects and opens the editor tab; the check button (or "Set active") makes it active; right-click menu as in 5.1. `+` creates "New Environment" and opens it with the name in edit mode.

**Environment editor tab.** Title (click to rename), colour dot picker (optional), toolbar (search, `...` with Duplicate, Export, Delete). Table with columns: enabled checkbox, **Variable**, **Value**, and optional **Description** column (toggle). A trailing placeholder row "Add variable" creates a row when you type. Per row: secret toggle (lock icon, masks the value with a reveal eye; hold Cmd/Ctrl to reveal all), delete on hover, drag handle to reorder (sorting by a column header disables manual order until cleared). Long values are truncated in the cell and expand into a multi-line popup editor on focus. Changes autosave (the environment tab never shows a dirty dot).

**Active environment selector** (top right, at the end of the tab strip): a button showing the active name and colour dot; a variables quick-look button beside it. Dropdown: search field, hint text about pinning (P2), `No Environment`, the list with a check on the active one, and `+ Create new environment` at the bottom. Choosing an item activates it immediately. The quick-look popover lists the variables of the active environment, the globals and the current request's collection variables (grouped, masked secrets, click a value to edit it in place).

**Globals.** Same table in a tab opened from the bottom bar ("Globals"). Lowest precedence.

**Scopes and precedence** (narrowest wins): global < collection < folder (outer to inner) < environment < data (runner iteration file) < local (script `pm.variables.set`, per run). Values are strings. A disabled row does not exist for resolution.

**Syntax and resolution.** `{{name}}`, case-sensitive; names may contain letters, digits, `_`, `-`, `.`, `$` (dynamic). Variables resolve in: URL, params, path variables, header keys and values, form fields, raw/GraphQL body, auth fields, gRPC URL/metadata/message, WebSocket URL/headers/messages, scripts through `pm.variables.replaceIn`. Resolution is recursive (a value may contain `{{other}}`), max depth 10, with a cycle guard that leaves the text unresolved and warns. Unresolved variables are left as literal text. If an unresolved variable remains in the URL's scheme/host, Send fails with a clear message that lists the unresolved names and the active environment (and never contacts a host named `{{x}}`).

**Editor integration.** In every single-line and multi-line field that accepts variables:
- Resolved `{{name}}` is drawn as a subtle chip; unresolved as a red-tinted chip. Typing `{{` opens an autocomplete list (variables from all scopes with a scope badge, plus dynamic variables), Tab/Enter accepts.
- Hovering a chip opens a popover: name, scope badge, current value (editable inline, writes to that scope), "Overridden" warning when a narrower scope shadows it, secret masking with reveal, and for unresolved ones an "Add to..." menu (environment, collection, globals; if no environment is active, offers to create one).
- Right-click on selected text: "Set as variable..." (name, value, scope dialog).

**Dynamic variables.** Resolved at send time, freshly per occurrence. See Appendix A for the v1 list (about 40) and exact formats.

**Scripts** can read and write variables (`pm.environment`, `pm.globals`, `pm.collectionVariables`, `pm.variables`), and `pm.environment.set` persists to the active environment file (secret values go to the secrets file). Local variables never persist. The UI shows a subtle indicator on the environment tab when a script changed it.

**Import/export** of environments and globals uses the Postman file shape (Appendix C). Tolerate missing `type`, `enabled` and `id`.

### 5.3 Requests

#### 5.3.1 Common

All three protocols share: name, description (Markdown "Docs" tab), breadcrumb (collection > folders > name; each segment clickable, name editable inline), `Save` (with a dropdown: Save As..., Save as example), save-to-collection dialog for unsaved requests, dirty tracking (a dot on the tab), duplicate/rename/delete from the tree, variables everywhere (5.2), a right-side code snippet panel.

**New request flow.** `+` in the tab strip, Cmd/Ctrl+T, or "New" opens an unsaved HTTP request named "Untitled Request" that lives only as a tab/draft. Saving asks for name and destination (collection > folder, with "New collection"). Unsaved drafts are persisted in the session file so a crash or quit never loses them. The protocol is chosen at creation and cannot change after the first save. "Autosave" is a setting (default off): when on, requests already inside a collection save after a short idle.

**Tabs of the request builder** (order): Docs, Params, Authorization, Headers, Body, Scripts, Settings; a `Cookies` link at the right opens the cookies manager. Count/dot indicators: Params and Headers show the number of enabled rows (Headers counts auto-generated ones in the table label like the screenshots do: "Headers 16"), Authorization shows a dot when a type other than No Auth is set, Body shows a dot when not `none`, Scripts shows a dot when either script is non-empty.

**Request line:** method dropdown (coloured text, no pill), URL field, Send button (blue, split with a dropdown: Send, Send and Download), while in flight the button becomes Cancel and shows an elapsed timer. Enter in the URL field sends. Pasting a cURL command into the URL field converts the whole request (an inline one-line hint offers Undo). A URL without a scheme gets `http://` prepended (this matches Postman; the resolved URL is visible in the Code snippet panel and Console). Autocomplete suggests URLs from other requests in the workspace.

##### 5.3.2 HTTP

**Send pipeline** (implement exactly in this order, in `engine`; each step is unit-testable):

1. Build the scope chain (collection, folders, request) and the variable context.
2. Run pre-request scripts: collection, folders outer to inner, request. They may change variables and the outgoing request; a script error stops the send and is shown with line number (scripts: 5.6). `pm.execution.skipRequest()` skips the send.
3. Resolve auth through the inheritance chain; resolve `{{vars}}` and dynamic variables in every field.
4. Build the URL: substitute `:pathVars`, add enabled query params, percent-encode per component (Appendix D), apply the scheme default. Honour the per-request "Encode URL automatically" setting.
5. Apply auth (it may add headers, query params or body fields; header-type results are shown, not saved).
6. Compute body and automatic headers (below). User-set headers always win over automatic ones with the same name; a user `Content-Type` wins over the generated one.
7. Attach cookies from the jar for the URL (merged with a manual `Cookie` header), unless the cookie jar is disabled.
8. Send: DNS, connect (proxy aware), TLS (ALPN: h2, http/1.1 per HTTP-version setting), write, read, decode (gzip/deflate/br/zstd), enforce max response size, record phase timings. Handle redirects manually (below). Handle Digest/NTLM challenge retry once, OAuth 2.0 token refresh before send.
9. Update the cookie jar from `Set-Cookie` of every hop.
10. Build the response model (status, reason, headers in received order and case, raw body bytes, decoded body, size breakdown, timings, HTTP version, TLS info and remote/local address).
11. Run post-response scripts and tests: collection, folders, request. Collect test results and console output.
12. Record: History entry, Console entries, UI update. Never write to the collection file because of a send.

**Automatic headers** shown (read-only, toggleable, with a hover explanation and a link to where they are controlled) in the Headers table when "auto-generated headers" are visible: `Host`, `User-Agent: Postguy/<version>`, `Accept: */*`, `Accept-Encoding: gzip, deflate, br`, `Connection: keep-alive`, `Content-Type` and `Content-Length` (derived from the body, value shown as `<calculated when request is sent>`), `Cookie` (from the jar), `Cache-Control: no-cache` (setting "Send no-cache header", default on), and auth-generated headers. Do not copy Postman's `Postman-Token`; Postguy sends no request-token header. The Headers tab has a "Hide auto-generated headers" toggle (default hidden shows a count), a Presets dropdown (named header sets; manage dialog; stored in settings), Bulk Edit (`key: value` per line, `//` prefix disables), header-name and common-value autocomplete.

**Params tab.** "Query Params" key/value/description table with enabled checkbox, placeholder row, Bulk Edit toggle (`key:value` lines), `...` menu. Below it "Path Variables" appear automatically for each `:name` segment in the URL path (value and description; never for `:` inside the host/port or after `?`). Editing either side updates the other immediately. A param with an empty key and value is kept as a row but ignored in the URL.

**Body tab.** Type selector: `none`, `form-data`, `x-www-form-urlencoded`, `raw`, `binary`, `GraphQL`.
- `form-data`: rows with Key, Value, type dropdown (Text / File), per-row Content-Type, description, enabled. File rows store a path relative to the working directory (setting) when inside it; files outside need the "allow files outside the working directory" setting (default off; GUI file-picker choices are copied into or referenced under the working directory as the user decides; imported collections that reference outside paths show an actionable error on send). Multipart boundary is generated; the Code tab and a "Preview request" view show the exact bytes of text parts.
- `x-www-form-urlencoded`: rows; keys and values are percent-encoded on send.
- `raw`: language dropdown Text, JavaScript, JSON, HTML, XML (sets `Content-Type` to `text/plain`, `application/javascript`, `application/json`, `text/html`, `application/xml` unless the user set one). JSON comments (`//`, `/* */`) are stripped on send. Beautify button and Cmd/Ctrl+Alt+B. Variables resolved inside strings; a JSON validity hint is shown but never blocks sending.
- `binary`: choose one file (working-directory-relative path), no content type is set automatically.
- `GraphQL`: Query editor + Variables editor (JSON). Sent as JSON `{"query": ..., "variables": ..., "operationName"?}` with `Content-Type: application/json`. P1. Schema introspection and field autocomplete are P2 (fetch with the standard introspection query on demand via a Refresh button).
- Switching the type never deletes data typed under another type until the request is saved with a different type (keep each mode's data in the model, write only the active one on export).

**Authorization tab.** Left column: "Auth type" dropdown; right: that type's form. Dropdown order and grouping (match the screenshots' structure with our own wording and icons): a top group "Inherited" showing the parent's resolved type with a check when selected; then API Key, Basic Auth, Bearer Token, Digest Auth, Hawk Authentication, JWT Bearer, OAuth 1.0, OAuth 2.0; then a separator and AWS Signature, NTLM Authentication, Akamai EdgeGrid, ASAP (Atlassian); then a separator and No Auth. When type is Inherit, the right side shows a heading "<type> Inherited", an "Edit in parent" link, and the parent's fields read-only in dashed-outline inputs. Field tables are in Appendix E. All fields accept variables. Generated headers/params are previewable (they appear in the Headers table as auto-generated rows) and are not saved.

**Settings tab** (per request; each has a one-line description in the UI and a "Restore default" affordance; defaults come from global settings): HTTP version (Auto, HTTP/1.x, HTTP/2), Enable SSL certificate verification, Automatically follow redirects, Follow original HTTP method (off: 301/302/303 on POST become GET), Follow Authorization header on cross-host redirects (off), Remove referer header on redirect, Maximum redirects (10), Request timeout ms (0 = none), Encode URL automatically (on), Disable cookie jar (off), Max response size MB. When "verify SSL" is off the response shows a visible warning chip.

**Redirects.** Manual loop. Each hop is recorded for the Console and the response timeline. Cross-host redirects drop `Authorization`, `Cookie` and body headers per the settings. 303 and (when not "follow original method") 301/302 on non-GET/HEAD switch to GET and drop the body. Loop detection and the max-hops cap are mandatory.

**Cancellation and timeouts.** Cancel button and Esc abort the in-flight request cleanly (connection dropped, UI back to idle, partial data discarded). Timeout 0 means no timeout.

##### 5.3.3 gRPC

New "gRPC" request. Layout (from the screenshots; our own styling): title row with the request name; request bar: **TLS lock toggle** (crossed lock = off) + **server URL** field + a vertical divider + **method selector** + **Invoke** button. Below: tabs **Docs, Message, Authorization, Metadata, Service definition, Scripts, Settings**; a green dot on Authorization/Metadata/Service definition when configured.

- **Server URL.** Accepts `host:port`, `grpc://host:port`, `grpcs://...`; variables allowed. TLS toggle off by default (Postguy infers on for `grpcs://` and port 443? No: explicit toggle only, `grpcs://` sets it on). Pasting a `grpcurl` command converts it (P2).
- **Method selector.** A dropdown button showing `Service / Method` with a type icon (unary, server-streaming, client-streaming, bidirectional; our own Lucide glyphs). The list is grouped by fully qualified service name (`package.Service` header, methods beneath, check mark on the selected one). Footer row: "Using <ServiceName>" + refresh button; (P2) "Generate collection" creates a collection with one request per method with example messages. When no definition is loaded the selector reads "Select a method" and the list explains how to load one.
- **Service definition tab.** Two sources: (a) **Import .proto file(s)**: file picker or drop, multiple files, an Import paths list (folders added as roots for bare `import "x/y.proto"` lines), the parsed result shown as a tree of services and methods, errors shown per file with line and column; files are copied into `<workspace>/protos/<name>/` and become a named **service definition** reusable by other requests (a dropdown lists existing ones, plus "open file"/"refresh" icons); (b) **Use server reflection** (`grpc.reflection.v1` first, fall back to `v1alpha`; same TLS/metadata/auth as the call; "Try again" button; failures explain what to check). The chosen source and its file list are stored in the request; a stale definition shows a refresh affordance, never silently changes.
- **Message tab.** JSON editor (line numbers, `{{var}}` chips) with Beautify, "Use Example Message" (generates JSON from the descriptor: zero values, first enum value, one element per repeated field, one entry per map, first oneof member, well-known types as their JSON forms (`Timestamp` RFC 3339, `Duration` `"1s"`), recursion guarded), and a saved-messages dropdown (name, load, save, delete). **Autocomplete**: inside the editor, typing a quote at a key position offers the field names of the request message type at the cursor's path (nested messages descend, enum values complete in value position), shown in a popup with a field icon and a chevron for message-typed fields (as in the reference screenshot); hovering a key or value shows its protobuf type in a tooltip. JSON mapping follows the proto3 JSON rules: message null = default, enum name or number, bytes as base64 or number array, 64-bit ints accepted as numbers or strings (output as strings), floats accept `"NaN"`/`"Infinity"`. Streaming modes: client/bidi streaming show **Send** and **End streaming** buttons in this tab after Invoke.
- **Authorization.** No Auth, API Key (header), Basic, Bearer, JWT Bearer, OAuth 2.0 (P1), Inherit. They produce metadata.
- **Metadata.** Key/value table like headers (enabled, description, variables). Keys are lowercased on the wire; `-bin` keys carry base64 values.
- **Settings.** Enable server certificate verification; Overwrite server name for certificate verification; Include fields with default values in the response (off); Maximum response message size MB (0 = unlimited); Connection timeout / deadline ms (0 = none).
- **Scripts.** Before invoke, On message, After response (P1 with scripting).
- **Invoke** flow: connect (HTTP/2, TLS if enabled, client certs from settings), send, then: unary = one response; server streaming = timeline of messages until completion; client streaming = user sends N messages, then End streaming; bidirectional = both. The button becomes Cancel while active.
- **Response pane.** Tabs: Response (messages), Metadata (initial headers), Trailers, Test Results. Header chips: status as `<code> <NAME>` (full table in Appendix F, coloured: OK green, others red/orange by class), time, size. Unary shows one pretty-printed JSON message. Streaming shows a **reverse-chronological timeline** with an entry per message: direction arrow (sent/received/info), local timestamp, size, expandable body; a connection-state indicator (connecting, streaming, closed); search box; filter All / Sent / Received; **Clear messages** with a Restore link; a ring buffer (cap default 10 000 messages or 64 MB) with a "N earlier messages dropped" row. Save response as an example only after the stream ends. `grpc-status-details-bin` is decoded against the pool and shown under the status.
- **Right panel code snippet** shows the equivalent grpcurl command.

##### 5.3.4 WebSocket

New "WebSocket" request. Request bar: URL (`ws://`, `wss://`; variables), **Connect / Disconnect** button. Tabs: **Message, Params, Headers, Settings** (Docs too). Params/Headers/Settings are read-only while connected (with a hint to disconnect).

- **Handshake.** Query params appended to the URL; custom headers (warn about managed ones: `Upgrade`, `Connection`, `Sec-WebSocket-Key`, `Sec-WebSocket-Version`); subprotocols field (`Sec-WebSocket-Protocol`); redirects followed (3xx) with the handshake timer reset; proxy and TLS as for HTTP.
- **Message composer** (bottom-left format selector: Text, JSON, XML, HTML, Binary; for Binary an encoding choice Base64 or Hex). Beautify for JSON/XML/HTML. **Send** (Cmd/Ctrl+Enter) keeps the text in the editor. **Save** stores the current message with the request; a saved-messages list (name, click to load, delete, add blank).
- **Messages pane.** Timeline (newest at bottom while following; "stick to bottom" unless the user scrolls up) of incoming, outgoing and network events (connected, closed with code and reason, ping/pong, error), each with direction arrow, local time, size and a first-line preview. Search, filter (All, Sent, Received, Network), Clear (with Restore). Expand a message to see it as Text/JSON/XML/HTML with wrap toggle, search, copy, "Show hexdump / Show message" toggle for binary. Select two messages to show the time difference. Per-message: Copy, Save to saved messages, info tooltip (size, time, opcode/MIME). Ring-buffer caps as gRPC.
- **Settings.** Enable server certificate verification; Handshake timeout ms; Reconnection attempts; Reconnection interval ms; Maximum message size MB (0 = unlimited). Ping/pong are answered automatically and shown as network rows. permessage-deflate is not supported in v1: say so in the docs.
- **Right panel code snippet** hidden for WebSocket in v1.
- Variables resolve at connect time for the URL and headers, and at send time for each message.

### 5.4 Response viewer

Shown below the request (or beside it in two-pane layout), resizable splitter, collapsible by dragging or a toggle. Empty state: a short, quiet line ("Send the request to see a response") and the shortcut hint; no illustration.

**Header strip** (left to right): tab-like switcher Body / Cookies / Headers / Test Results / Timeline (counts as small superscripts; Test Results shows passed/total; the strip collapses to a dropdown when narrow, as in the narrow-layout screenshot), then right-aligned status chips: status `200 OK` (green 2xx, blue 3xx, orange 4xx, red 5xx, red "Could not send request" for transport errors), time (`602 ms`), size (`7.63 KB`), a network/lock icon, `Save Response` menu, `...` menu (Save to file, Clear response, Copy response). Hovering time shows the phase breakdown (Prepare, DNS, TCP, TLS, Request sent, Waiting/TTFB, Download); hovering size shows headers vs body (and request sizes); hovering the network icon shows local/remote address, HTTP version, TLS version, cipher, certificate subject/issuer/validity and warnings (self-signed, expired, hostname mismatch, verification disabled).

**Body.** Mode switch **Pretty | Raw | Preview | Hex**. Pretty has a language dropdown (JSON, XML, HTML, YAML, JavaScript, Markdown, Text; auto from `Content-Type`, then by sniffing), line numbers, syntax colours, code folding for JSON/XML (P2), wrap toggle, search (Cmd/Ctrl+F: case, regex, whole-word toggles, count and next/prev), copy, and a **filter** field (JSONPath for JSON, XPath for XML/HTML, P1; yellow underline = no match, red = invalid expression). URLs in bodies are clickable (Cmd/Ctrl+click opens a new GET tab). Preview renders images (PNG/JPEG/GIF/WebP/SVG-as-image via our decoders only if already in the tree; otherwise "image/xyz, N bytes, Save to file"), shows JSON/XML as a collapsible tree, plain text, and for HTML shows the source (no web view). PDF/audio/video show metadata and "Save to file". Hex view is a virtualised 16-bytes-per-row dump with ASCII gutter.

**Large bodies** (non-negotiable): the body is stored as bytes; a line-start index is built off the UI thread; only visible lines are laid out (virtualised, cached `LayoutJob` per line keyed by line and theme); highlighting is lazy per visible line; JSON pretty-printing uses a token-level re-indenter on a worker with a "Formatting..." state and never builds a full value tree; above 5 MB the default view is Raw without highlighting with an opt-in "Pretty" button; above the max-response-size setting the response is stopped, truncated with a banner, and offers "Save to file" / "Raise limit". Search runs on a worker and returns offsets.

**Cookies tab** lists cookies set by this response (name, value, domain, path, expires, HttpOnly, Secure). **Headers tab** lists received headers (name/value, in received order, hover info icon with a one-line explanation for common headers). **Test Results tab**: Passed/Failed/Skipped filter, PASS/FAIL badges with assertion messages, "Refresh results" (re-run post-response scripts on the stored response). **Timeline** (P2) shows redirect hops and requests actually sent (request line, headers, body preview).

**Save Response.** "Save as example" (needs the request to be in a collection; stores request snapshot and response, body capped at 5 MB with a message when truncated), "Save to file" (raw bytes, suggested file name from `Content-Disposition`/URL). **Send and Download** streams the body straight to a chosen file without holding it in memory.

**Response history for a saved request**: a dropdown of previous responses (time, status) is P2; the History panel (5.5) is the P1 mechanism.

### 5.5 Cookies, certificates, proxy, console, history, trash

**Cookies manager** (opened from the `Cookies` link, the bottom bar and the command palette): two tabs, *Manage cookies* (domains list; per domain list of cookies; add domain; add cookie from a template `Name=value; Path=/; Expires=...`; edit in place; delete cookie, delete domain, clear all; supports Domain, Path, Expires/Max-Age, HttpOnly, Secure, SameSite is stored and shown but not enforced for sending decisions beyond RFC 6265bis defaults) and *Domain allowlist* (domains scripts may read/write through `pm.cookies.jar()`). The jar persists in `.postguy/cookies.json`. Global "disable cookies" and per-request "disable cookie jar" settings exist. Runner has "run without stored cookies" and "save cookies after run".

**Certificates and TLS** (Settings > Certificates): optional CA bundle PEM (multiple CAs); client certificates per host pattern (`*.example.com`, optional port, default 443): CRT+KEY files or a PFX plus optional passphrase; stored in the config dir (passphrases are not written to disk in v1: ask on first use per session, or document the limitation); duplicates: last added wins; sent only over HTTPS. "Enable SSL certificate verification" global default on. TLS 1.2 minimum. Optional SSLKEYLOGFILE support (P2).

**Proxy** (Settings > Proxy): use system proxy (on by default; honours `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` and OS settings where discoverable), or a custom proxy: apply to HTTP and/or HTTPS, protocol HTTP / SOCKS5 / SOCKS5h (SOCKS4/4a P2), host, port, optional basic auth, bypass list (comma-separated, wildcard hosts). Custom wins over system. A proxy forces HTTP/1.x for plain HTTP. The effective proxy and certificates used are logged in the Console.

**Console** (bottom bar button, Cmd/Ctrl+Alt+C; docked panel, optionally detachable later): shows, newest at the bottom, every request sent (method, URL, status, time; expandable to resolved request headers/body, redirect hops, proxy, certificate used, network info, response headers/body), script `console.log/info/warn/error` output, warnings (for example SSL verification off, unresolved variables) and errors. Toolbar: filter chips (All, Requests, Logs, Warnings, Errors), search, timestamps toggle, Clear. Ring buffer 5000 entries / 24 h, virtualised, bodies truncated to 1 MB in the log with a note. Secret variable values are masked. Local only.

**History** (sidebar section, collapsed by default; HISTORY header): grouped by day, newest first; each row shows method label, URL, time; click reopens a snapshot in a new tab; hover actions Save (to a collection) and Delete; `...` menu: Clear all, "Save responses" toggle (default off; when on, bodies are cached under `.postguy/responses/` and pruned with the entry). Entries come from user sends only, never from the runner. Cap 5000 (setting).

**Trash**: deleting collections, folders, requests and environments moves them to `.trash/` with metadata. Bottom bar Trash opens a list (search, Restore, Delete permanently, Empty trash, retention setting). A single undo (Cmd/Ctrl+Z) for the most recent tree operation (delete, move, rename, duplicate, reorder) while the app is open.

### 5.6 Automation: scripts, tests, runner, CLI, agent tools

"Automation" in Postguy means four things that share one engine: scripts and tests attached to requests, the collection runner, a headless CLI runner, and agent control (tools, MCP, UI control). All are local.

#### 5.6.1 Scripts and tests (P1)

- **Where.** Collection, folder and request each have **Pre-request** and **Post-response** scripts (the Post-response script is where tests live; in files it is stored under the event name `test` for Postman compatibility). gRPC adds Before invoke, On message, After response. Editors have line numbers, syntax colouring, bracket matching, Beautify, a snippets menu (Appendix G), and autocomplete for `pm.` members.
- **Order.** Pre-request: collection, folders outer to inner, request. Post-response: same order. Each script runs to completion before the next; a thrown error is reported (script name, line, message), skips the remaining scripts of that phase and, for pre-request, aborts the send.
- **Runtime.** `boa_engine` in the `script` crate, one fresh context per script run (no state leaks between runs except variables). Async: `await` and Promises work; the host pumps the job queue. Limits: wall-clock timeout (default 5 s per script, setting), loop-iteration and recursion limits, memory cap, no filesystem, no process, no network except `pm.sendRequest`. Scripts from imported collections run in this sandbox; a global "Disable scripts" setting and a runner "skip scripts" option exist. A hostile script must not be able to hang or crash the app.
- **The `pm` API (v1 subset, implemented natively in Rust host objects; no JS prelude files).**
  - Variables: `pm.variables`, `pm.environment`, `pm.globals`, `pm.collectionVariables` with `has/get/set/unset/clear/toObject/replaceIn`; `pm.environment.name`; `pm.iterationData` (`has/get/toObject/unset`). `pm.variables.set` creates a local variable. `replaceIn` resolves dynamic variables too.
  - `pm.request`: `url` (with `toString()`, `getHost()`, `getPath()`, `getQueryString()`, `query`, `addQueryParams`, `removeQueryParams`), `headers` (`add/remove/upsert/get/has/toObject`), `method`, `body` (read), `auth` (read). Header and URL edits in pre-request scripts take effect.
  - `pm.response` (post-response): `code`, `status`, `headers` (`get/has`), `responseTime`, `responseSize`, `text()`, `json()`, `cookies`, and `pm.response.to.*` helpers: `have.status(code|text)`, `have.header(name[, value])`, `have.body(text)`, `have.jsonBody([path[, value]])`, `have.responseTime.below(ms)`, `have.jsonSchema(schema)`, `be.ok`, `be.success`, `be.error`, `be.clientError`, `be.serverError`, `be.json`, `be.withBody`, `not.*`.
  - `pm.test(name, fn)`, `pm.test.skip`, `pm.test.index()`; `pm.expect(value)` with a Chai-BDD-compatible subset implemented in Rust: `to.equal/eql/deep.equal`, `to.be.a/an(type)`, `to.be.true/false/null/undefined/empty/ok`, `to.be.above/below/least/most/within`, `to.include/contain`, `to.have.property`, `to.have.length/lengthOf`, `to.have.keys/members`, `to.match`, `to.be.oneOf`, `.not`, `.deep`. Failure messages follow Chai's wording (`expected 404 to equal 200`).
  - `pm.info` (`eventName`, `iteration`, `iterationCount`, `requestName`, `requestId`), `pm.execution.skipRequest()` (pre-request only), `pm.execution.setNextRequest(name|id|null)` (runner only, P2 for loops), `pm.sendRequest(urlOrRequest[, callback])` (P2), `pm.cookies` read access plus `pm.cookies.jar()` for allow-listed domains (P2).
  - Globals: `console.log/info/warn/error/clear`, `setTimeout` (capped), `atob/btoa`, `TextEncoder/Decoder`, `URL/URLSearchParams`, `JSON`, `Math`, `Date`, and a small `require()` that serves only modules implemented in Rust as host functions: `uuid`, and a `crypto-js`-compatible subset (MD5, SHA1/256/512, HMAC, Base64, Hex). Any other module makes `require` throw a clear "module not available offline" error. (Do not bundle npm libraries; bundling JS would break the Rust-only rule. If a JS prelude proves unavoidable for some API, stop and ask the owner.)
  - Not in v1: `pm.vault`, `pm.require` of packages, `pm.visualizer`, `pm.mock`, legacy `postman.*`/`tests[]` globals (provide a small compatibility shim for `postman.setEnvironmentVariable/getEnvironmentVariable/setGlobalVariable/setNextRequest` and `tests["name"] = bool` only if cheap).
- **Test results** are collected per request (name, passed/failed/skipped, message, duration). A passing test counts once even with several assertions; the first failing assertion fails the test. Console output of scripts goes to the Console panel.
- **Errors in scripts** show in the response area as a "Script error" banner with the phase, script level (collection/folder/request) and line, and in the Console.

#### 5.6.2 Collection runner (P1)

**Entry points:** collection or folder `Run` (context menu, collection detail header), the bottom bar `Runner`, the command palette. Opens a **Runner tab** that mirrors the design of the Postman runner but local only.

**Configuration pane:**
- Source (collection or folder), environment (defaults to the active one, can change), request list with checkboxes (select all/none, shift-click range), drag handles to reorder for this run, "Run order" note (folder contents before root requests).
- Iterations (default 1), Delay between requests ms (default 0), Data file (CSV or JSON, with a preview table; CSV first row = variable names; JSON = array of flat objects; one iteration per row; data variables resolve as the local scope), Persist responses for this session (default on; bodies over 300 KB are not kept), Stop run if an error occurs (default on: script exception or transport failure; a failed *test* does not stop the run), Keep variable values after the run (default off: variable changes made by scripts during the run are discarded), Run without stored cookies (default off), Save cookies after run (default off), Turn off console logging (default off), Skip scripts (default off).
- **Run** and **Cancel** buttons. `pm.execution.setNextRequest` jumps only inside whole-collection runs; a folder run is limited to that folder (P2).

**Results pane:** summary header (source, environment, iterations, duration, total/passed/failed tests, errors, average response time), iteration selector, per-request rows (status chip, method, name, tests passed/failed, time, error badge NETWORK / REQUEST / SCRIPT), click a row for details (resolved request, response headers/body if persisted, test results, console lines). Tabs: All, Passed, Failed, Errors, Console. Actions: Run again, New run (back to config), Export results (JSON), Delete run. Completed runs are saved under the collection's **Runs** tab (summary only; bodies only if persisted and small) and capped (default 50).

Never run requests concurrently inside one run. The UI stays responsive; results stream in as they complete.

#### 5.6.3 CLI (`postguy-cli`)

```
postguy-cli run <collection.json | workspace-collection-name> [options]
  -e, --environment <file|name>     -g, --globals <file>
  -d, --iteration-data <csv|json>   -n, --iteration-count <n>
  --folder <name|id>  (repeatable)  --delay-request <ms>
  --env-var k=v   --global-var k=v  --timeout-request <ms>  --timeout-script <ms>
  --bail [failure|folder]           -k, --insecure     --ignore-redirects
  --cookie-jar <file> --export-cookie-jar <file>
  --ssl-client-cert/-key/-passphrase  --ssl-extra-ca-certs <pem>
  --working-dir <dir>  --no-insecure-file-read
  -r, --reporters cli,json,junit,html      --reporter-<name>-export <path>
  --reporter-cli-no-summary / -silent / -no-failures / -no-assertions / -no-console / -show-timestamps
  --reporter-json-omit-request-bodies / -omit-response-bodies / -omit-headers
  --color on|off|auto   --silent   --verbose   -x, --suppress-exit-code
postguy-cli tools | call <tool> --json '{...}' | run --script steps.json --root DIR
postguy-cli import <file|curl-string> [--into <collection>]   postguy-cli export <collection> [-o file]
postguy-cli snippet <request-ref> --lang curl|fetch|python|go|rust|powershell|node-axios|grpcurl
postguy-cli mcp [--root DIR]        # opt-in MCP server (stdio)
postguy-cli ui --control FILE <inspect|click|key|type|set|command|screenshot>
```

Exit code 0 when all tests pass and no errors; 1 on test failure or runtime error (unless `-x`); 2 on bad arguments. The cli reporter prints per-request lines (method, name, status, time, size), per-test PASS/FAIL lines and a final table (iterations, requests, test-scripts, prerequest-scripts, assertions: executed and failed; total duration, total data received, average response time) followed by a numbered failure list. JSON reporter: collection, environment, run{stats, timings, executions[], failures[]}. JUnit: one suite per request/folder, one case per test. HTML: single self-contained file (inline CSS only, generated by Rust, no JS).

#### 5.6.4 Agent tools and control (AGENTS.md section 3)

Everything user-facing has an automation tool: `snake_case` names, JSON-Schema inputs, unknown arguments rejected, errors say what was wrong and what is allowed, writes atomic, all paths honour `--root`, no unsaved work discarded silently. Tool families (initial set; `postguy-cli tools` lists the real ones):

`workspace_open/info`, `collection_list/create/rename/duplicate/delete/export`, `folder_create/rename/move/delete`, `request_create/get/update/move/duplicate/delete` (all three protocols), `request_send`, `request_cancel`, `response_get/save/render_body`, `env_list/create/rename/duplicate/delete/set_active/get/set_var/unset_var/export`, `globals_get/set_var`, `variables_resolve` (explain how a string resolves and where each variable came from), `cookies_list/set/delete/clear`, `history_list/open/clear`, `console_get/clear`, `import_file/import_text/import_curl`, `snippet_generate`, `grpc_load_definition/describe/example_message/invoke`, `ws_connect/send/messages/disconnect`, `run_collection`, `run_results_get`, `settings_get/set`, `script_run` (evaluate a script against a request for testing), `trash_list/restore/empty`. Each tool has an end-to-end test and a `command` link where a UI command exists.

The UI control channel runs only with `--control FILE`, binds to loopback, uses a fresh random token per launch in an owner-only file, is never on by default and never logs the token. MCP runs only when a user launches `postguy-cli mcp`; never started by the app, CLI or installers; any future network transport is loopback plus token.

### 5.7 Imports and exports

**Import dialog** (Cmd/Ctrl+I, sidebar import button, command palette, drag and drop anywhere on the window): one text box that accepts pasted text, a cURL command or a URL to a file (fetching a URL is a user action and shown as such), a "Choose files" button (multiple), and a drop zone for files and folders. Format is auto-detected; if several files are given, a checklist lets the user pick. Result: an **Import report** (what was imported, counts, warnings such as "auth type X mapped to Y", "unsupported field kept as extras", "script uses unsupported API"), then "Open". Importing never overwrites silently: name clashes get a numeric suffix; "Replace existing" is an explicit option for same-id imports.

| Format | Tier | Notes |
|---|---|---|
| Postman Collection v2.1 and v2.0 | P0 | Folders, requests, bodies (raw, urlencoded, formdata, file, graphql), auth blocks, events (prerequest/test scripts), variables, examples (`response[]`), `protocolProfileBehavior`. v1 collections give a clear "convert to v2 first" message. gRPC/WebSocket items in newer exports are imported best-effort if recognised, otherwise listed in the report. Unknown fields kept in `extras`. Postman v3 YAML (P3) is detected and reported as unsupported with instructions |
| Postman environment and globals (`values[]`, `_postman_variable_scope`) | P0 | `type: secret` becomes a secret variable; `enabled: false` kept |
| cURL | P0 | Pasted into the URL bar or import box. Parse: URL, `-X/--request`, `-H/--header`, `-d/--data/--data-raw/--data-binary/--data-urlencode/--data-ascii`, `-F/--form` (text and `@file`), `-u/--user`, `-A/--user-agent`, `-b/--cookie`, `-e/--referer`, `-G/--get`, `--url`, `-L/--location`, `-k/--insecure`, `--compressed`, `--http2/--http1.1`, `-I/--head`, `-x/--proxy`, `--max-redirs`, `-m/--max-time`, `--json`; line continuations `\` and `^`, single/double/ANSI-C `$'...'` quoting; multiple commands give multiple requests. Unknown flags are reported, never fatal |
| OpenAPI 3.0/3.1, Swagger 2.0 (JSON and YAML) | P1 | Folders by tag (default) or by path; request name from summary, operationId, then method+path; `servers[0].url` becomes a collection variable `baseUrl` and URLs use `{{baseUrl}}`; path/query/header params with example or schema-generated values (generation depth-limited, cycle-guarded, `$ref` resolved with cycle detection, external refs not fetched); request body from `examples`/`example`/schema for the first content type; `securitySchemes` map to collection-level auth (http basic/bearer, apiKey, oauth2) with requests inheriting; response examples become Examples; deprecated operations included but marked. Options dialog: folder strategy, preferred body type, include deprecated |
| HAR 1.2 | P1 | Each entry becomes a request; cookies, headers, post data, query. Skip pseudo-headers |
| `.proto` files | P1 | Imported as a gRPC service definition (see 5.3.3) |
| Postman data dump (folder with collections/environments), Insomnia v4 JSON | P2 | |

**Exports:** collection to Postman v2.1 JSON (secrets stripped, `info._postman_id`, `schema` URL, scripts as `event`, auth as arrays of `{key,value,type}`), collection to Postguy native, environment and globals (Postman shape; secret values omitted unless the user ticks "include secret values"), run results (JSON, JUnit, HTML), all workspace data as a zip (P1), code snippets, a single response body. Round-trip tests: Postman v2.1 import then export must preserve every field the schema defines and every unknown field it carries (golden files written by us).

Hostile input caps (AGENTS.md section 4): max file size (default 100 MB, setting), max JSON nesting 256, max items per collection 200 000, max YAML aliases expansion, max OpenAPI schema example depth 8, all with clear errors.

### 5.8 Code snippets

Right sidebar panel (toggle in the bottom bar, Cmd/Ctrl+Alt+K) for HTTP and gRPC requests: language dropdown, gear (indent size and type, "include boilerplate"), Copy button, read-only highlighted view that updates live as the request changes. Variables are resolved against the active environment (a toggle "keep variables" shows `{{var}}` unresolved). v1 languages: cURL, raw HTTP/1.1, JavaScript (fetch), Python (requests), Go (net/http), Rust (reqwest), PowerShell (Invoke-RestMethod), NodeJS (axios), grpcurl. cURL output style: `curl --location --request POST 'https://host/path?x=1' \` then `--header 'K: V' \` and a body flag (`--data-raw`, `--form 'k="v"'`, `--form 'f=@"/path"'`, `--data-urlencode`), one flag per line. Write golden tests per language. "Copy as cURL" exists in request context menus and as a tool.

---

## 6. User interface

### 6.1 Principles

Look: calm, flat, breathable. 1 px hairline separators, few boxes, no drop shadows except popups, 8 px spacing grid, row height 30 px (tree) and 34 px (tables), tab strip 40 px, bottom bar 36 px, control radius 6 px (cards 8 px), sans-serif UI text at 14.5 px with 12.5 px small text and 20 px headings, light grey sidebar on white content (dark theme: layered charcoal). Hover reveals secondary actions. Section labels are muted small caps. No banners, no promotional text, no emoji, no illustrations (empty states are one line of muted text plus one primary action).

Do not reuse Postman's brand orange, artwork, icons, fonts, copy or layout pixel values. Use our own semantic tokens (below) and Lucide icons. Method colours follow universal REST conventions (green GET, amber POST, blue PUT, violet PATCH, red DELETE) as text colours in our own palette, tuned for contrast in both themes.

### 6.2 Window layout

```
+------------------------------------------------------------------------------------------+
| header: [<][>]  workspace-name v      [ search / command palette  Cmd+K ]         [gear]  |
+--------------------+---------------------------------------------------+-----------------+
| sidebar (300px)    | tab strip: [POST Face Live*][v1][Runner][+][v]  [env v][vars]       |
| [filter__][+][imp][...]  +---------------------------------------------+-----------------+
| COLLECTIONS  v     | breadcrumb ................................. [Save v]  | right panel     |
|  > Face Service Go | [METHOD v][ url ..................... ][Send v]       | (optional)      |
|  v Face Service Rust| Docs Params Authorization Headers Body Scripts Settings | Code snippet  |
|     v v1           |  request editor ...                                  |                 |
|        POST Debug  +------------------- splitter ------------------------+                 |
| ENVIRONMENTS v     | response: [Body Cookies Headers Tests]  200 OK 602ms 7.6KB [Save v]|                 |
|  Face Service Rust ok|  response viewer ...                                |                 |
| HISTORY >          |                                                     |                 |
+--------------------+-----------------------------------------------------+-----------------+
| bottom: [sidebar] [Console] [Cookies] [Runner]            [Globals] [Trash] [two-pane][right]|
+------------------------------------------------------------------------------------------+
```

- Native window decorations. Min size 900x560, default 1280x800, remembered per session (position clamped to visible monitors). Splitters are draggable and persisted; double-click resets.
- **Header (46 px):** back/forward (tab history), workspace label (click: Open Workspace..., Recent, Reveal in file manager), centred search field that opens the command palette overlay, settings gear. Nothing else.
- **Sidebar:** top row: filter field with filter icon, `+` (New menu: HTTP Request, gRPC Request, WebSocket Request, Collection, Environment), Import button, `...` menu (Collapse all, Sort, Show/hide History section). Then stacked sections **COLLECTIONS**, **ENVIRONMENTS**, **HISTORY**, each with a collapsible header (chevron + small caps label), a splitter between sections, independent scrolling and persisted open/closed state. No other sections ever (no APIs, Specs, Mocks, Datasets, Flows, Toolsets, Documents).
- **Tab strip:** tabs for requests (method label or protocol badge + name + close/dirty dot), collections, folders, environments, globals, runner, runner results, settings, cookies, trash. Preview tab is italic. Unsaved dot replaces the close button until hover. Drag to reorder, middle-click closes, overflow chevron opens "Search tabs" (open and recently closed, Cmd/Ctrl+Shift+A). Context menu: New Request, Duplicate Tab, Close Tab, Force Close Tab, Close Other Tabs, Close All Tabs, Force Close All Tabs, Reveal in Sidebar. Closing a dirty tab asks Save / Don't save / Cancel (setting "always ask", default on). Cmd/Ctrl+Shift+T reopens the last closed tab. At the right end of the strip: environment selector and variables quick-look (5.2).
- **Right panel (optional, 340 px):** Code snippet (5.8). Toggle from the bottom bar. Hidden by default.
- **Bottom bar:** left: sidebar toggle, Console, Cookies, Runner; right: Globals, Trash, two-pane toggle, right-panel toggle. A small status string on the far right (workspace saved/saving state). No version/marketing text.
- **Request/response layout:** response below the request (default) or side by side (two-pane toggle, Cmd/Ctrl+Alt+V). A request in flight shows a thin indeterminate bar under the request line, not a spinner overlay.

### 6.3 Components to build (in `ui-egui/src/widgets`)

- Square checkbox, outlined radio, toggle switch, split button, dropdown/select with check marks and grouped headers and separators, searchable popup list, context menu with icons and shortcut hints, tooltip, inline status messages (no toasts or popups), modal dialog, confirm dialog (typed confirmation variant), inline-rename text edit.
- **KeyValueTable:** enabled checkbox, key, value, description columns; placeholder row; drag reorder; per-row delete on hover; Bulk Edit toggle; `...` menu; read-only auto-generated rows with info tooltip; virtualised for 1000+ rows; paste of multi-line `key:value` or `key=value` text splits into rows; secret column variant; file-or-text value variant (form-data).
- **Field with variables** (single-line): chips for `{{var}}`, autocomplete, hover popover, URL-aware behaviour (tab/enter, paste cURL), reduced allocations (layout cached by text hash).
- **CodeEditor** (multi-line): line numbers, current-line highlight, token colouring (JSON, XML, HTML, JS, GraphQL, YAML, plain, proto), bracket and quote auto-close and matching, auto-indent, Tab handling, Beautify, find bar, autocomplete popup anchored with `galley.pos_from_cursor` that consumes Tab/Enter/Up/Down before the text edit sees them, hover tooltips, `{{var}}` chips, soft limit that disables highlighting for huge text, edit history, folding (P2). Highlight cache keyed by a revision counter, not by re-hashing the whole text per frame.
- **LargeTextView** (read-only viewer for responses): virtualised lines, search, selection and copy across lines, wrap off by default, hex mode.
- **Timeline** (gRPC/WebSocket/console): virtualised, reverse or follow-bottom, expandable rows, search and filters.
- **Tree** (collections, trees in service definitions, JSON tree): flattened visible list cached and invalidated on model/expansion change, virtualised, keyboard navigation, drag and drop with indicators, multi-select.
- **Command palette:** fuzzy search over commands, requests, collections, environments, open tabs; prefix `>` for commands only.

### 6.4 Interaction rules

- Every action is a registered command with an id, label, default shortcut and enabled predicate; menus, palette, shortcuts, control channel and automation all use the registry.
- Never block: dialogs for file choice run off the UI thread (`rfd` async).
- Inline rename: Enter commits, Esc cancels; empty names are rejected with the previous name kept.
- Destructive actions: delete goes to trash with an "Undo" hint in the bottom-bar status area; permanent delete asks for confirmation.
- Focus: after Send, focus stays where it was. Cmd/Ctrl+L focuses the URL field.
- Accessibility: egui accesskit enabled, full keyboard operability for tree, tabs, tables and dialogs, minimum contrast 4.5:1 for text in both themes, UI scale setting (Cmd/Ctrl +/-/0) from 80% to 200%.
- Empty states (examples): collections empty: "No collections yet" + `Create collection` and `Import`; response empty: "Send the request to see a response."; tests empty: "No tests ran for this request."

### 6.5 Keyboard shortcuts (CmdOrCtrl = Cmd on macOS, Ctrl elsewhere)

| Action | Shortcut |
|---|---|
| Send / Invoke / Connect | CmdOrCtrl+Enter |
| Send and Download | CmdOrCtrl+Alt+Enter |
| Cancel in-flight | Esc (when the request area is focused) |
| Save / Save As | CmdOrCtrl+S / CmdOrCtrl+Shift+S |
| New request tab | CmdOrCtrl+T |
| New (menu: request, collection, environment) | CmdOrCtrl+N |
| Close tab / force close | CmdOrCtrl+W / CmdOrCtrl+Alt+W |
| Reopen closed tab | CmdOrCtrl+Shift+T |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab |
| Go to tab 1-8 / last | CmdOrCtrl+1..8 / CmdOrCtrl+9 |
| Search tabs | CmdOrCtrl+Shift+A |
| Command palette / search | CmdOrCtrl+K, CmdOrCtrl+Shift+P |
| Focus URL | CmdOrCtrl+L |
| Find in editor/response | CmdOrCtrl+F |
| Find in workspace (P2) | CmdOrCtrl+Shift+F |
| Toggle sidebar / right panel | CmdOrCtrl+\ / CmdOrCtrl+Alt+K |
| Two-pane toggle | CmdOrCtrl+Alt+V |
| Console | CmdOrCtrl+Alt+C |
| Import | CmdOrCtrl+I |
| Settings | CmdOrCtrl+, |
| Beautify | CmdOrCtrl+Alt+B |
| Duplicate / Copy / Cut / Paste (tree) | CmdOrCtrl+D / C / X / V |
| Rename / Delete (tree) | F2 or Enter / Delete |
| Zoom in / out / reset | CmdOrCtrl+= / - / 0 |
| Undo / redo (editor or tree) | CmdOrCtrl+Z / CmdOrCtrl+Shift+Z |
| Comment line (script editor) | CmdOrCtrl+/ |

Shortcuts must not collide with OS shortcuts; the table lives in one place and the Settings Shortcuts tab (P2) edits it.

### 6.6 Theme tokens

Light, Dark and System (follow the OS live on macOS and Windows; XDG portal on Linux; light fallback). Tokens (semantic names; hex are starting points, tune by eye and check contrast): `surface` (#FFFFFF / #1E1F22), `surface-alt` (#F6F7F9 / #25272B), `surface-sunken` (#EEF0F3 / #191A1D), `border` (#E2E5EA / #34363B), `text` (#1F2328 / #E6E8EB), `text-muted` (#6A7380 / #9AA1AB), `accent` (#2F6FEB / #5B93F5) for primary buttons, links, focus rings, selected tab underline, `success` (#1F9D55 / #4CC38A), `warning` (#B7791F / #E0A458), `danger` (#D64545 / #F07178), `info` (#2B7DB8 / #6BB3E8), method colours `get` (#1F9D55), `post` (#B7791F), `put` (#2F6FEB), `patch` (#7C5CC4), `delete` (#D64545), `head` (#2B9C8F), `options` (#C2549B) with lighter dark-theme variants. Status chips use tinted backgrounds of success/info/warning/danger. Syntax colours are a small set derived from these. Variable chips: accent-tinted (resolved) and danger-tinted (unresolved).

### 6.7 Typography and icons

egui default fonts for v1 (UI 14.5 px sans-serif, 12.5 px small, 20 px headings; code 13.5 px from the editor-font-size setting; all scaled by the UI scale), system font fallback at runtime for missing glyphs, right-to-left and bidi-safe file names where cheap (as pdfcraft does with unicode-bidi, optional). Icons: Lucide, converted at build time to path data, drawn at 14-16 px with 1.5 px strokes, one `icons.rs` with an enum, all attributed. Method labels and status chips are text, not icons.

---

## 7. Settings

A Settings **tab** (Cmd/Ctrl+,) with a left list: General, Themes, Shortcuts (P2), Data, Certificates, Proxy, About. All changes apply immediately and persist to `settings.json` (unknown keys preserved; corrupt file falls back to defaults).

**General** (defaults in parentheses): HTTP version (Auto); Request timeout ms (0 = none); Max response size MB (50, 0 = unlimited); SSL certificate verification (on); Disable cookies (off); Automatically follow redirects (on); Send no-cache header (on); Trim keys and values in tables (off); Response format detection (Auto); Working directory (`<workspace>/files`); Allow reading files outside the working directory (off); Always open sidebar items in a new tab (off); Always ask when closing unsaved tabs (on); Show icons with tab names (on); Two-pane layout (off); Variable autocomplete (on); Autosave (off); Editor: font size (13.5), indent size (2 for JSON, 4 else) and type (spaces), auto-close brackets and quotes (on), word wrap in editors (off); History: keep entries (5000), save responses (off); Trash retention days (30); Disable scripts (off); Script timeout s (5); UI scale (100%).
**Themes:** System / Light / Dark.
**Data:** open workspace folder location, Export all data (zip), Import data (zip or folder), Reveal in file manager, Reset settings (not data).
**Certificates:** CA bundle (toggle + PEM path), client certificates list with add/remove.
**Proxy:** as 5.5.
**About:** version, commit, licence texts of the project and a generated third-party licence list (`cargo xtask licenses`). No update check, no network.

---

## 8. Performance and size budgets

Targets (tracked by `cargo xtask bench` and `cargo xtask size`; CI fails on regressions beyond the stated margin; measure on a mid-range laptop and record the machine):

| Metric | Budget |
|---|---|
| Release binary, stripped, macOS arm64, `scripting` on | <= 18 MB (stretch 14 MB); `scripting` off <= 12 MB |
| Cold start to first interactive frame | <= 250 ms (stretch 150 ms), session with 10 tabs restored lazily |
| Idle CPU (window focused, nothing happening) | ~0%: no repaint when nothing changes; blinking caret and spinners request repaints only while visible |
| Idle RSS, empty workspace | <= 90 MB; 1000-request workspace <= 140 MB |
| Open a collection of 10 000 requests | tree usable <= 300 ms; filter keystroke <= 16 ms |
| Send overhead (our code, not network) on loopback | <= 5 ms from click to first byte written; <= 16 ms from last byte to rendered Pretty view for a 100 KB JSON body |
| 10 MB JSON response | first Raw frame <= 200 ms; Pretty formatted off-thread in <= 1.5 s; scrolling at 60 fps; memory <= 3x body size |
| 100 MB response with limit raised | no UI freeze; stays within 2x body size; cancel works |
| 50 000 timeline messages | scrolling at 60 fps (virtualised); ring buffer enforced |
| Frame time | <= 8 ms typical, never > 33 ms from our code in interactive paths |

How:
- Release profile: `opt-level = "s"` (measure `3` for hot crates via per-package overrides: JSON tokenizer, line indexer, search), `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `debug = false`, **`panic = "unwind"`** (required by AGENTS.md section 4). Dev profile as pdfcraft (opt-level 1, dependencies 2).
- `default-features = false` everywhere, no `syntect`, no `resvg`, no `regex` (use `regex-lite`), no `chrono`, avoid duplicate dependency versions (`cargo tree -d` in CI), `cargo bloat` report in `xtask size`.
- egui discipline: reactive repaint only; reuse buffers; keep big bodies in `Arc<[u8]>`/`Arc<str>`; cache galleys and `LayoutJob`s by (text revision, theme); virtualise anything with more than ~100 rows with `ScrollArea::show_rows` or a viewport + prefix-sum height cache; precompute tab titles and method labels; limit font sizes and families.
- Do all parsing, formatting, searching, importing, hashing and script execution off the UI thread with cancellation.

---

## 9. Security and robustness

- AGENTS.md section 4 in full: `Result` everywhere, no panicking shortcuts, no `unsafe`, checked arithmetic, `get()` not indexing, bounded recursion and allocations, lock-poisoning handled, `catch_unwind` guards on import, script and command entry points, every crash fix has a regression test that failed first.
- Untrusted inputs to treat as hostile: imported collections/OpenAPI/HAR/cURL strings, `.proto` files, reflection responses, HTTP responses (headers, chunking, compression bombs: cap decoded size and ratio, redirects), WebSocket frames, gRPC messages and descriptors (recursion, huge `repeated`), cookie strings, URLs, scripts, data files, settings files, workspace files edited by other tools.
- Scripts: sandboxed, time- and memory-limited, no fs/process/network (except `pm.sendRequest`, P2, which goes through the same http engine and Console).
- File access: form-data/binary/proto/data-file paths resolve against the working directory; escaping it requires the setting; automation honours `--root` and refuses every outside path with the same message whether or not it exists.
- TLS: verification on by default; turning it off is visible (chip, console warning). Client cert passphrases and secret values are never logged. The Console masks secret variable values. Control channel and MCP are opt-in only.
- Credentials in `Authorization` are shown in the Console (needed for debugging); secret-typed variable values are masked everywhere they are echoed.
- Network egress: the app makes no requests the user did not initiate. No analytics, no crash upload, no update check. A test asserts that launching the app and opening a workspace opens no sockets.
- Crash recovery: autosave of drafts into `session.json` every few seconds when dirty; on start after a crash, restore tabs and drafts and say so once.

---

## 10. Testing and verification

Tests mirror four zooms: property, unit, crate integration, end-to-end.

- **Hermetic:** tests never touch the internet. Provide `crates/testkit` with: a loopback HTTP server (hyper) that echoes requests, serves redirects, cookies, digest/basic/bearer challenges, slow/streaming/compressed/huge bodies, TLS with a generated self-signed cert, an OAuth2 token endpoint, a tonic server built from our own original `echo.proto` (unary and all streaming modes, reflection, status codes, metadata, trailers), and a WebSocket echo server (text/binary, ping/pong, close codes, subprotocols, slow handshake).
- **Property tests:** variable resolver (no panics, depth/cycle caps, idempotence on text without braces), URL <-> params round trip, percent-encoding per component, cURL parse then generate then parse, Postman v2.1 import/export round trip, fractional ordering, JSON token formatter (output parses to the same value; invalid input never panics).
- **Fuzz:** `cargo xtask fuzz` mutation-fuzzes the importers (Postman, OpenAPI, HAR, cURL), URL parser, cookie parser, JSONPath/XPath filters, `.proto` parser wrapper, variable resolver and script sandbox in child processes; findings go to a git-ignored folder and become synthetic regression tests before fixing.
- **UI:** `egui_kittest` tests for tree interactions, context menus, tabs, dirty tracking, env selector, key-value tables, editors, autocomplete; the control channel is tested like pdfcraft's `tests/control.rs`. Visual checks: launch with `--control` and use `postguy-cli ui ... screenshot`; headless `shot` example for CI screenshots of our own UI with demo data.
- **End-to-end:** each automation tool has a test that runs through `Automation::call` against the testkit servers (send, redirects, cookies, auth types, import/export, gRPC invoke modes, WebSocket session, runner with data file, scripts and tests).
- **Fidelity:** golden files for exports and snippets in `crates/*/tests/golden/`, written by us.
- **Performance:** `xtask bench` builds synthetic workspaces and bodies and checks the budgets in section 8 using frame-time capture via the headless harness.
- **Process:** prove a test fails before the fix by reverting the fix. Review every change at the four zooms in AGENTS.md section 5.2. Scope builds with `-p <crate> -j 2` under `timeout`.

Agents verify their own work by driving the engine with `postguy-cli run --script` / MCP tools and by launching the app with `--control` and taking screenshots of the real UI.

---

## 11. Milestones

Each task is one commit with its id (for example `M2.3: redirect loop with cookie capture`). Keep `plan/STATUS.md` (current milestone, next unchecked task, blockers) and `ROADMAP.md` (one-page summary, progress log) up to date at the end of every session. Create `plan/execution-plan.md` from this section with the autonomous-operation protocol (orient, plan, implement + test, verify, record, commit). Ask the owner only for decisions listed in section 13 as theirs.

**M0 Bootstrap (no features).**
- M0.1 `git init`; `.gitignore` (`target/`, `dist/`, `plan/`, `prompt_images/`, `fuzz-out/`, `.DS_Store`); move `prompt_images/` to `plan/postman/`; `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`, `README.md`, `ROADMAP.md`, `deny.toml`, `clippy.toml`, `rustfmt.toml`, `.cargo/config.toml`.
- M0.2 Workspace `Cargo.toml` and empty crates with READMEs and lint headers; `xtask` with `ci`, `layers`, `assets`, `parity`, `deny`, `size`, `icons`, `fuzz` skeletons; `ATTRIBUTION.toml`/`ATTRIBUTION.md` generation; CI workflow (fmt, clippy `-D warnings`, tests, layers, assets, deny) on macOS, Linux and Windows.
- M0.3 Spikes with ADRs in `plan/adr/`: glow vs wgpu, hyper-direct vs reqwest, boa vs rquickjs (run a pm-style script, measure size and speed), icon pipeline, JSON line-index viewer prototype. Decide with numbers.
- M0.4 `plan/STATUS.md`, `plan/execution-plan.md`, `parity/postman-features.toml` skeleton (one entry per feature in this document; statuses planned), `docs/` skeleton (`architecture.md`, `development.md`, `roadmap.md`, `gaps.md`).

**M1 Model, variables, storage (v0.1 part 1).** `model` types and serde with `extras`; `util`; `urlkit` (parse/build/encode/sync); `vars` resolver and dynamic variables; `store` workspace with atomic writes, lock, trash, secrets split, session/settings; `interop` Postman v2.1/v2.0 and environment/globals import and export with `ImportReport` and round-trip goldens; cURL parse/generate; automation skeleton with `workspace_*`, `collection_*`, `folder_*`, `request_*`, `env_*`, `import_*`, `export_*` tools and tests.

**M2 HTTP engine (v0.1 part 2).** hyper connector with timings, TLS verify toggle, custom CA, HTTP/1.1 and h2 via ALPN, compression, size cap, cancellation; body building (all modes); automatic headers; Basic/Bearer/API key; manual redirects with settings; cookie jar with persistence and `Set-Cookie` handling; proxy (env, custom HTTP/SOCKS5); the send pipeline in `engine` (without scripts); history and console models; `request_send` and `cookies_*`, `history_*`, `console_*` tools; testkit servers; `postguy-cli run --script` can send requests.

**M3 Desktop shell and HTTP UI (v0.1 part 3).** eframe app, theme tokens, icon pipeline, widgets (5.x list), header, sidebar (collections tree with all interactions and context menus, environments, history), tab strip with preview/dirty/closed-tab handling, session restore and drafts, request builder (URL field with chips and autocomplete, Params, Headers, Body, Authorization for P0 types, Settings, Docs), response viewer (Pretty/Raw/Preview/Hex, large-body strategy, search, headers/cookies/timing popovers), environment editor and selector, globals, collection/folder detail tabs, save flows, trash, command registry and palette, console panel, cookies manager, settings tab, bottom bar, code snippet panel with cURL/fetch/requests, shortcuts, control channel and `postguy-cli ui`. Screenshots reviewed against the section 6 layout. Budgets from section 8 measured for everything that exists.

**M4 Imports and snippets (v0.1 part 4).** Import dialog with drag-and-drop and report; cURL paste into URL bar; all remaining snippet languages; export dialogs; all-data zip; golden tests. Tag **v0.1**.

**M5 Auth types.** Digest (challenge retry), JWT Bearer, OAuth 2.0 (all grants; browser authorization using a loopback redirect listener opened only on demand and closed after; PKCE; token storage per request/collection with "Manage tokens"; auto-refresh), OAuth 1.0, AWS Signature v4, Hawk, client certificates (PEM and PFX), request Settings tab complete, GraphQL body mode and introspection (basic). NTLM, EdgeGrid, ASAP only if time permits (P2).

**M6 WebSocket (v0.2 part 1).** `ws` crate, UI, saved messages, settings, reconnect, tools and tests.

**M7 gRPC (v0.2 part 2).** `grpc` crate: protox compile with import paths, descriptor pool, dynamic codec, unary then streaming modes, reflection (v1 then v1alpha), example generator, completion data, status/trailers, settings, TLS and client certs, service definitions in `protos/`, UI (selector, tabs, timeline), saved messages, examples, grpcurl snippet, tools and tests. Tag **v0.2**.

**M8 Scripts and tests (v0.3 part 1).** `script` crate with `pm` API (5.6.1), Chai-compatible `expect`, test results UI, script editors with autocomplete and snippets, Console integration, limits and fuzzing, script hooks for gRPC. Pipeline steps 2 and 11 wired in.

**M9 Runner and CLI (v0.3 part 2).** `runner` crate, Runner tab (config, results, history), data files, `postguy-cli run` with reporters, OpenAPI and HAR importers, `.proto` import dialog, opt-in MCP server complete, parity file brought up to date. Tag **v0.3**.

**M10 Polish and 1.0.** Budget enforcement in CI, accessibility pass, UI zoom, shortcuts customisation (if time), packaging scripts (macOS universal `.app` + dmg, Windows MSI and portable zip, Linux tar.gz/deb/AppImage, FreeBSD pkg) with our original app icon, release workflow, docs (user guide generated from our UI), licence list, security review, final gap list. Tag **v1.0**.

---

## 12. Docs, parity and tooling

- `parity/postman-features.toml`: one `[[feature]]` per user-facing feature (id, title, area, tier, milestone, status = planned|partial|shipped|na, `commands`, `tools`, `evidence` = paths of passing tests, `notes` saying what is missing). `cargo xtask parity` validates that cited commands, tools and tests exist. "Shipped" means exists and tested, not "as good as Postman"; do not mark shipped on a generic test. `na` marks cloud-only Postman features with the reason.
- `docs/`: `architecture.md`, `development.md`, `roadmap.md`, `gaps.md`, `file-formats.md` (native format and Postman mapping), `scripting.md` (supported `pm` API), `cli.md`, `mcp.md`, `settings.md`, each with a status line and revision history like pdfcraft's progress docs.
- `cargo xtask`: `ci`, `layers`, `assets`, `parity`, `deny`, `size`, `bench`, `fuzz`, `icons`, `licenses`, `screenshots`, `version`.
- Every crate README states layer, status, API, conventions.

---

## 13. Decisions already made (change only with a written reason) and the owner's decisions

Made here (agent may proceed): glow first; hyper-direct HTTP; boa for scripts with the `pm` API in Rust and the `scripting` feature flag; one JSON file per collection behind a `store` trait; no web/WASM target; no dependency on `craft-ui`/`craft-libs`; egui default fonts + runtime system fallback; secrets in a local file (not encrypted) in v1; autosave off; no request-token header; Lucide icons rendered from build-time path data; Postman v2.1 as the interchange format.

The owner's (ask before doing): any weakening of `AGENTS.md` rules; adding any font file or choosing where fonts live; the app name/bundle identifier, logo and domain; switching to `rquickjs` or `reqwest`; adding any dependency whose licence is not on the allowlist; anything that adds network traffic the user did not initiate; enabling MCP/control channel by default (never).

---

## 14. Appendices

### Appendix A. Dynamic variables (v1)

Resolved per occurrence. Names are case-sensitive and start with `$`.

- Common: `$guid` and `$randomUUID` (UUID v4 text), `$timestamp` (Unix seconds), `$isoTimestamp` (ISO 8601 UTC with ms, `2026-10-19T08:10:36.177Z`).
- Numbers/text: `$randomInt` (0-1000), `$randomAlphaNumeric` (one char), `$randomBoolean`, `$randomHexColor` (`#47594a`), `$randomColor` (colour name), `$randomPrice` (0.00-1000.00), `$randomSemver`.
- Internet: `$randomIP`, `$randomIPV6`, `$randomMACAddress`, `$randomPassword` (15 alphanumerics), `$randomUserAgent`, `$randomProtocol`, `$randomDomainName`, `$randomDomainSuffix`, `$randomDomainWord`, `$randomUrl`, `$randomEmail`, `$randomExampleEmail`, `$randomUserName`.
- People/places: `$randomFirstName`, `$randomLastName`, `$randomFullName`, `$randomNamePrefix`, `$randomNameSuffix`, `$randomPhoneNumber`, `$randomCity`, `$randomStreetName`, `$randomStreetAddress`, `$randomCountry`, `$randomCountryCode`, `$randomLatitude`, `$randomLongitude`, `$randomJobTitle`, `$randomCompanyName`.
- Dates: `$randomDateFuture`, `$randomDatePast`, `$randomDateRecent` (ISO format), `$randomWeekday`, `$randomMonth`.
- Finance: `$randomCurrencyCode`, `$randomCurrencyName`, `$randomCurrencySymbol`, `$randomBankAccount`, `$randomBitcoin`.
- Text: `$randomWord`, `$randomWords`, `$randomPhrase`, `$randomLoremWord`, `$randomLoremWords`, `$randomLoremSentence`, `$randomLoremSentences`, `$randomLoremParagraph`, `$randomLoremParagraphs`, `$randomLoremText`, `$randomLoremSlug`.
- Files: `$randomFileName`, `$randomFileExt`, `$randomFileType`, `$randomMimeType`, `$randomFilePath`.

Generators use small word lists written for this project (contributor-original data; record them in `ATTRIBUTION.toml` as original) and `getrandom`/`fastrand`. Exact strings need not match any other tool; shapes and ranges must. Image-URL variables and the rest of the long tail are P3.

### Appendix B. Auto-detected languages and content types

JSON (`application/json`, `+json`), XML (`application/xml`, `text/xml`, `+xml`), HTML, JavaScript, YAML, Markdown, CSS, plain text; images by `Content-Type` and magic bytes; everything else Raw/Hex. Sniff only the first 4 KB. "Response format detection: force JSON" overrides.

### Appendix C. Postman file shapes to read and write

- Environment/globals: `{ "id", "name", "values": [ { "key", "value", "type": "default|secret|any", "enabled": true } ], "_postman_variable_scope": "environment|globals", "_postman_exported_at", "_postman_exported_using" }`. Read tolerantly (missing `type` = default, missing `enabled` = true).
- Collection v2.1: `info { _postman_id, name, description, schema }`, `item[]` (folder: `name`, `description`, `item[]`, `auth`, `event[]`, `variable[]`; request item: `name`, `request`, `response[]`, `event[]`), `request { method, header[], body { mode: raw|urlencoded|formdata|file|graphql, raw, urlencoded[], formdata[], file, graphql { query, variables }, options.raw.language }, url { raw, protocol, host[], port, path[], query[], variable[] } or string, auth, description }`, `auth { type, <type>: [ { key, value, type } ] }` with types apikey, awsv4, basic, bearer, digest, edgegrid, hawk, noauth, oauth1, oauth2, ntlm (plus jwt/asap read-through as unknown-typed blocks and preserved), `event[] { listen: prerequest|test, script { type, exec[] } }`, `variable[] { key, value, type, disabled }`, `response[] { name, originalRequest, status, code, header[], cookie[], body, _postman_previewlanguage }`, `protocolProfileBehavior`. `disabled` flags on header/query/urlencoded/formdata/variable rows map to `enabled: false`. Vendor the official v2.1 schema as test data to validate our exports.
- Auth attribute keys on import/export: apikey `key,value,in`; basic `username,password`; bearer `token`; digest `username,password,realm,nonce,nonceCount,algorithm,qop,opaque,clientNonce,disableRetryRequest`; hawk `authId,authKey,algorithm,user,nonce,extraData,appId,delegation,timestamp`; awsv4 `accessKey,secretKey,region,service,sessionToken,addAuthDataToQuery`; oauth1 `consumerKey,consumerSecret,token,tokenSecret,signatureMethod,timestamp,nonce,version,realm,addParamsToHeader,includeBodyHash,addEmptyParamsToSign`; oauth2 `grant_type,authUrl,accessTokenUrl,clientId,clientSecret,scope,redirect_uri,state,tokenName,addTokenTo,headerPrefix,client_authentication,useBrowser,refreshTokenUrl,accessToken`; ntlm `username,password,domain,workstation`; edgegrid `accessToken,clientToken,clientSecret,baseURL,nonce,timestamp,headersToSign`. Verify against real exports captured from a Postman install with synthetic data before trusting them.

### Appendix D. URL encoding by component

Encode on send (when "Encode URL automatically" is on): path: `"`, `<`, `>`, `` ` ``, `#`, `?`, `{`, `}`, space; query: `"`, `#`, `&`, `'`, `<`, `=`, `>`, space (a value's own `&`/`=` are encoded; the structural ones are not); userinfo: `"`, `<`, `>`, `` ` ``, `#`, `?`, `{`, `}`, `/`, `:`, `;`, `=`, `@`, `[`, `\`, `]`, `^`, space. Existing valid `%XX` sequences are left alone. Non-ASCII is UTF-8 percent-encoded; IDN hosts converted with IDNA. A text selection context menu offers EncodeURIComponent / DecodeURIComponent.

### Appendix E. Auth forms (fields; every field accepts variables)

- **No Auth**: nothing. **Inherit**: see 5.3.2.
- **API Key**: Key, Value, Add to (Header | Query Params).
- **Basic**: Username, Password. `Authorization: Basic base64(user:pass)`.
- **Bearer**: Token (prefix `Bearer`; custom prefix via API Key named `Authorization`).
- **Digest**: Username, Password; Advanced: Realm, Nonce, Algorithm (MD5, MD5-sess, SHA-256, SHA-256-sess, SHA-512-256...), qop (auth, auth-int), Nonce Count, Client Nonce, Opaque; checkbox "Disable retrying the request". Flow: send, on 401 with `WWW-Authenticate: Digest`, compute and retry once.
- **JWT Bearer**: Add to (Header | Query), Algorithm (HS256/384/512, RS*, ES*, PS*), Secret (+ "Secret is Base64 encoded") or Private key (PKCS#8 PEM, pasted or file), Payload (JSON editor), Advanced: header prefix (default `Bearer`), extra JWT headers. Token is generated at send time (`iat` added unless present).
- **Hawk**: Hawk Auth ID, Auth Key, Algorithm (sha256/sha1); Advanced: User, Nonce, ext, app, dlg, Timestamp, "Include payload hash".
- **OAuth 1.0**: Add to (Request headers | Request body/URL), Signature method (HMAC-SHA1, HMAC-SHA256, HMAC-SHA512, RSA-SHA1, RSA-SHA256, RSA-SHA512, PLAINTEXT), Consumer key/secret, Access token, Token secret (RSA: private key); Advanced: Callback URL, Verifier, Timestamp, Nonce, Version (1.0), Realm, Include body hash, Add empty params to signature, Encode params in header.
- **OAuth 2.0**: Add auth data to (Request headers | URL), Header prefix (`Bearer`); Current token (Access Token field + type); **Configure new token**: Token name, Grant type (Authorization Code, Authorization Code with PKCE, Implicit, Password Credentials, Client Credentials), Callback URL, Authorize using browser (loopback listener on 127.0.0.1, random port, closed afterwards), Auth URL, Access Token URL, Client ID, Client Secret, Scope, State, Client authentication (Basic header | body), PKCE (method S256/plain, verifier), Username/Password for password grant, Advanced: Refresh Token URL, extra params for auth/token/refresh requests (key, value, send in body/URL/headers). Buttons: Get New Access Token, Use Token, Refresh; token list with Manage (delete); auto-refresh toggle. Tokens stored in the secrets file, never exported.
- **AWS Signature (v4)**: Add to (Headers | URL), AccessKey, SecretKey, Region (`us-east-1`), Service name, Session token.
- **NTLM** (P2): Username, Password, Domain, Workstation, disable retry. **Akamai EdgeGrid** (P2): Access token, Client token, Client secret, optional nonce/timestamp/base URI/headers to sign/max body. **ASAP** (P2): Algorithm, Issuer, Audience, Key ID, Private key, Subject, Additional claims, Expiry (3600 s).

### Appendix F. gRPC status codes

0 OK, 1 CANCELLED, 2 UNKNOWN, 3 INVALID_ARGUMENT, 4 DEADLINE_EXCEEDED, 5 NOT_FOUND, 6 ALREADY_EXISTS, 7 PERMISSION_DENIED, 8 RESOURCE_EXHAUSTED, 9 FAILED_PRECONDITION, 10 ABORTED, 11 OUT_OF_RANGE, 12 UNIMPLEMENTED, 13 INTERNAL, 14 UNAVAILABLE, 15 DATA_LOSS, 16 UNAUTHENTICATED. Common failure hints: UNAVAILABLE often means TLS is off but the server needs it (or the server is down); reflection failures name which of v1/v1alpha were tried.

### Appendix G. Script snippets menu

Get/set/unset/clear an environment, global or collection variable; increment a counter variable; status code is 200; status code name contains text; response body contains string / equals string; JSON value check; response time below N ms; Content-Type header check; parse JSON and store a field in a variable; send a request (P2); skip request; set next request (P2).

### Appendix H. Observed reference layout (from the private screenshots; behaviour only, never copy visuals)

Use these notes to match structure, not pixels or artwork.

- **Sidebar:** single column; top icon row and filter field; `+`, import and `...` buttons; stacked uppercase section headers COLLECTIONS / ENVIRONMENTS / more (we keep only three); nested outline folder icons; request rows begin with a tiny method label; examples nested under requests with a small "example" icon; selected row filled; the active environment row shows a filled check badge on the right; a focused environment row has an outlined border.
- **Tab strip:** small method label + truncated name + orange/primary dot for unsaved; italic for preview; folder, collection, runner and environment tabs use icons; `+` and overflow chevron; environment selector pinned at the right end with a variables quick-look icon beside it.
- **Collection detail:** title, a "last updated" line with a history icon, a description paragraph (Markdown), and tabs Overview / Authorization (dot when set) / Scripts / Variables (dot when set) / Runs; top-right Run button. We drop Publish docs, Share, Fork/use counters, "You" avatar and team banners.
- **Environment tab:** header with a checkbox-style "active" toggle and name; two-column table Variable / Value with a search icon and `...` in the header; per-row small type/visibility indicators; a trailing "Add variable" placeholder row; long values truncated with an ellipsis.
- **Env selector dropdown:** search field, a hint about pinning an environment to a collection, a highlighted "No Environment" row, environment names with a check on the active one, and "Create new environment" at the bottom.
- **Request page:** breadcrumb at top-left with the request name in bold; Save button with a dropdown caret, Share (dropped); method select with coloured text; URL with highlighted `{{variable}}` chip; blue Send button with a split caret; tabs Docs, Params, Authorization, Headers (count, green dot when set), Body (green dot), Scripts, Settings, and a right-aligned Cookies link.
- **Params and headers tables:** header row with Key / Value / Description columns and "Bulk Edit" plus a `...` menu at the far right; a muted placeholder row; in Headers a "Hide auto-generated headers" toggle chip, a Presets dropdown, per-row info icons, auto-generated values shown as `<calculated when request is sent>`.
- **Body selector:** dropdown listing none, form-data, x-www-form-urlencoded, raw, binary, GraphQL; with raw selected, a second blue dropdown shows the language (JSON).
- **Authorization tab:** left "Auth type" dropdown with grouped entries and icons (Inherited group first, then API Key, Basic Auth, Bearer Token, Digest Auth, Hawk Authentication, JWT Bearer, OAuth 1.0, OAuth 2.0; then AWS Signature, NTLM, Akamai EdgeGrid, ASAP; then No Auth); right side shows the form, or for Inherited a heading with an "Inherited" tag, an "Edit in Parent" link and dashed read-only inputs.
- **Response:** strip with Body dropdown (narrow layout), status chip (`200 OK` green, `400 Bad Request` red), time, size, network icon, "Load test" (dropped), Save Response, `...`; below: JSON language dropdown, Preview, Visualize (dropped), toolbar icons (wrap, filter, search, copy, link), line-numbered syntax-coloured JSON. The code snippet panel at the right shows the language dropdown (cURL), a gear and copy button, and the generated command.
- **gRPC:** tab/page title with a protocol badge and the URL variable as the name; request bar = lock icon with variable chip URL, divider, method selector `Service / Method` with a unary icon, Invoke button; the method dropdown lists a service header (`package.Service`) and methods with unary icons and a check on the selected one, a footer "Using <Service>" with refresh and "Generate collection" (P2); tabs Docs, Message, Authorization (green dot), Metadata, Service definition, Scripts, Settings; Service definition shows a service dropdown with open/refresh icons, a link "Import a .proto file", "OR", and "Use Server Reflection"; Message editor has a "Compose message" placeholder and a popup of field names with wrench-like field icons and a chevron on message-typed fields.
- **Bottom bar:** sidebar toggle, folder, Terminal (dropped), Console on the left; Globals, Vault (dropped), Tools (dropped), layout toggles on the right.
- **Things in the screenshots to cut:** team/plan banner, Invite, Upgrade, notification bell, avatar, workspace switcher, AI, Share, Fork, Publish docs, Load test, Visualize, sync/cloud icons on variables, Spec Hub migration banners, APIs/Specs/Mocks/Datasets/Flows/Toolsets/Documents sections, the astronaut illustration.

