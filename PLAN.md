# PLAN.md: execution plan for Postguy

This plan turns the milestones in `PROMPT.md` (section 11) into an order of work, with exit criteria, review stops and risks. `AGENTS.md` binds every step. `PROMPT.md` says what to build; this file says in what order and how to check it. Keep this file true: tick tasks off, record changes of order with a reason.

Status legend: `[ ]` not started, `[~]` in progress, `[x]` done.

---

## 0. Starting point and decisions

**State at plan time:** the folder holds `AGENTS.md`, `PROMPT.md`, `PLAN.md` and `prompt_images/`. No git repo, workspace or code. Toolchain: `rustc`/`cargo` 1.98 nightly, macOS arm64. `cargo-deny` and `cargo-bloat` are not installed.

**Pattern source:** `../pdfcraft` (`xtask/src/{assets,layers,parity,fuzz,screenshots,version}.rs`, `.cargo/config.toml`, `.github/workflows`, `clippy.toml`, `deny.toml`, `crates/automation`, `crates/testkit`). Reuse as patterns only. Never depend on its `craft-*` crates.

**Decisions from the owner:**

| Topic | Decision |
|---|---|
| Protocol order | HTTP first (M2-M5), then WebSocket (M6), then gRPC (M7) |
| Fonts | egui default fonts plus the runtime system-font fallback (AGENTS.md 1.4). No font files committed |
| Name | Display name `postguy`. Crate and binary prefix `postguy-`. Bundle id placeholder `dev.postguy.app` (changeable before packaging) |
| Logo | A single "P" in a techie style, built as original vector geometry (angular or circuit-like strokes), recorded in `ATTRIBUTION.toml` as contributor-original. It must not resemble any Postman mark |
| Review | The owner reviews at each tag (v0.1, v0.2, v0.3, v1.0) and at the end of M0 and M3. Stop and wait at those points |
| Real Postman | Installed on the owner's macOS machine. Used for black-box observation only (see section 2) |

**Open until the owner answers:** permission to `cargo install cargo-deny cargo-bloat` (otherwise `xtask deny` and `xtask size` implement the checks themselves), and confirmation that `prompt_images/` moves to `plan/postman/` in M0.1.

**Resolution recorded 2026-10-19 (M0 session, defaults per this file):** the tools were not installed, so `xtask deny` (licence/source checks; cargo-deny still runs in CI) and `xtask size` implement the checks themselves; `prompt_images/` moved to `plan/postman/` as this file and AGENTS.md specify. The script-engine memory-cap gap is a new owner decision, recorded in ADR 0004 and `plan/STATUS.md`.

---

## 1. Working method (every task)

1. One task id per commit (`M2.3: redirect loop with cookie capture`), on a branch `<type>/<slug>`. Commit only green states. `main` is never pushed to directly.
2. Order inside a feature: engine API, then automation tool, then UI.
3. Every change has a test at the right zoom (property, unit, crate integration, end to end). A bug fix has a test seen failing first, by reverting the fix.
4. Review each change at the four zooms (AGENTS.md 5.2): clause, function, module, project.
5. Gates before each commit, scoped to the crates touched and run under `timeout`: `cargo fmt --check`, `cargo clippy -p <crate> -j 2 --all-targets -- -D warnings`, `cargo test -p <crate> -j 2`, `cargo xtask layers`, `cargo xtask assets`. The full workspace sweep is CI's job.
6. End of every session: update `plan/STATUS.md`, tick this file, add a line to the `ROADMAP.md` progress log, update the matching `parity/postman-features.toml` entry (shipped only with a passing test cited as evidence).
7. UI work: look at the result (`postguy --control FILE`, then `postguy-cli ui ... screenshot`, or the headless `shot` example) before calling it done.
8. Budgets (PROMPT.md section 8) are measured at the end of every milestone, not only at M10.

---

## 2. Observing Postman safely (owner's install)

Rules (AGENTS.md section 2): synthetic data only; never open the app bundle (`app.asar`, bundled JS, resources); notes and any screenshots live only under `plan/postman/` (gitignored), never traced, linked or committed; real exports are kept local.

How it works: the agent cannot drive the owner's Postman GUI, so for each check it writes exact steps, and the owner pastes back text or an export made from a throwaway collection with fake data. Results are recorded as text notes in `plan/postman/observations.md`.

Planned observation requests:

| When | Ask the owner to | Used for |
|---|---|---|
| M1 | Export (v2.1) one throwaway collection holding one fake request per auth type, a folder with auth/script/variable, a form-data and a urlencoded body, an example, and a disabled row of each kind; export an environment containing a secret variable and a disabled variable; export globals | Auth attribute keys (Appendix C), `protocolProfileBehavior` keys, `disabled` flags, secret `type`, exact field names |
| M2 | Send fake requests to a local echo server and paste the received headers and the Console entry | Automatic header set and order, redirect behaviour, cookie handling |
| M3 | Read out Settings > General defaults, the Shortcuts list, tab context-menu items, and the auto-generated headers table | Defaults and shortcut table (PROMPT.md 6.5, 7) |
| M6/M7 | Export a WebSocket and a gRPC request (fake) | Non-HTTP item shape in exports |

---

## 3. Phases and tasks

### Phase 0: M0 Bootstrap (about 1-2 days, no features)

- [x] **M0.1 Repo hygiene.** `git init`. `.gitignore` (`target/`, `dist/`, `plan/`, `prompt_images/`, `fuzz-out/`, `.DS_Store`). Move `prompt_images/` to `plan/postman/`. `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`, `README.md`, `ROADMAP.md`, `deny.toml`, `clippy.toml` (allow unwrap/expect/panic in tests; `disallowed-methods` for egui checkbox/radio), `rustfmt.toml` (`max_width = 150`, `use_small_heuristics = "Max"`), `.cargo/config.toml` (`xtask` alias).
- [x] **M0.2 Workspace skeleton.**
  - Root `Cargo.toml`: `resolver = "3"`, edition 2024, `MIT OR Apache-2.0`, `unsafe_code = "forbid"`, release profile (`opt-level = "s"`, `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "unwind"`), dev profile like pdfcraft.
  - Empty crates, each with README and the never-crash `#![deny(...)]` header: `model`, `util`, `vars`, `urlkit`, `auth`, `interop`, `store`, `http`, `grpc`, `ws`, `script`, `snippets`, `runner`, `engine`, `platform`, `ui-egui`, `automation`, `testkit`. Apps `postguy`, `postguy-cli`.
  - `xtask`: `ci`, `layers`, `assets`, `parity`, `deny`, `size`, `icons`, `fuzz`, `screenshots`, adapted from pdfcraft. `layers` encodes the PROMPT.md 4.1 table.
  - `ATTRIBUTION.toml` plus generated `ATTRIBUTION.md`; entries for egui's default fonts as Cargo-dependency assets.
  - CI workflow (fmt, clippy `-D warnings`, tests, layers, assets, deny) on macOS, Linux, Windows; PR template.
- [x] **M0.3 Spikes, each ending in an ADR in `plan/adr/` with measured numbers.**

| Spike | Method | Pass bar |
|---|---|---|
| glow vs wgpu | Same demo window on each; stripped binary size, cold start to first frame, idle CPU and RSS | Pick glow unless it fails on a supported OS |
| hyper-direct vs reqwest | Connector with phase timings and a manual redirect; confirm tonic can share the same hyper | Keep hyper if timings, header order/case and redirect control work in about 500 lines; else reqwest with the reason written down |
| boa vs rquickjs | pm-style script (`pm.test` + `expect`): time, binary delta, timeout and loop-limit behaviour | Keep boa unless it fails correctness or speed. Changing needs owner approval |
| Icons | `xtask icons`: Lucide SVG to path data, drawn with the egui painter; also the "P" logo as vector geometry | No SVG renderer in the binary |
| Large-JSON viewer | 10 MB body, off-thread line index, virtualised rows | First frame within 200 ms, 60 fps scrolling |

- [x] **M0.4 Tracking docs.** `plan/STATUS.md`, `plan/execution-plan.md` (derived from this file), `parity/postman-features.toml` (one `planned` entry per feature in PROMPT.md), `docs/{architecture,development,roadmap,gaps}.md`.
- [x] **M0.5 Logo.** Original "P" mark: SVG source, PNG sizes via `xtask`, `ATTRIBUTION.toml` entry as contributor-original, with the check that it differs from any Postman mark.

**Exit criteria:** `cargo xtask ci` green on the empty workspace; ADRs written; a window opens with the spike renderer. **REVIEW STOP 1 (owner).**

### Phase 1: v0.1 (M1-M4)

#### M1 Model, variables, storage, Postman import/export

- [x] M1.1 `util`: size-capped readers, depth-limited JSON walker, ring buffer, fractional ordering, atomic file helpers.
- [x] M1.2 `model`: all types in PROMPT.md 4.5, serde, `extras` preservation, ids.
- [x] M1.3 `urlkit`: parse/build, per-component percent-encoding (Appendix D), query to params two-way sync, path variables, host-pattern matching. Property tests.
- [x] M1.4 `vars`: scopes and precedence, recursive resolver (depth 10, cycle guard), dynamic variables (Appendix A, word lists written for the project), secret masking. Property tests.
- [x] M1.5 `store`: workspace layout, atomic writes with `.bak`, lock file with stale detection, trash, secrets split, history file, settings and session files with corrupt-file handling, change detection.
- [x] M1.7 `interop` cURL: parse (all flags in PROMPT.md 5.7) and generate. Property test: parse, generate, parse.
- [ ] M1.6 `interop` Postman: v2.0/v2.1 collection import and export, environment and globals, `ImportReport`, size/nesting caps, round-trip goldens written by us, vendored v2.1 schema as test data (`postman_data = true` entry). Observation request 1 (section 2).
- [ ] M1.8 `automation` skeleton and `postguy-cli tools|call|run --script`: `workspace_*`, `collection_*`, `folder_*`, `request_*`, `env_*`, `import_*`, `export_*`; `--root` confinement with the identical refusal for existing and missing paths. End-to-end tests.
- [ ] M1.9 Fuzz harness for importers, URL and cookie parsers, resolver (`xtask fuzz`).

**Exit:** Postman v2.1 round trip preserves every schema-defined and unknown field; exports validate against the official schema; fuzz runs clean for 5 minutes; crates clean under the never-crash lints.

#### M2 HTTP engine and send pipeline

- [ ] M2.1 `testkit`: loopback HTTP server (echo, redirects, cookies, basic/bearer/digest challenges, slow/streaming/compressed/huge bodies), self-signed TLS, token endpoint.
- [ ] M2.2 `http` connector: DNS/TCP/TLS with phase timings, rustls (native roots plus optional CA), verify toggle, ALPN h1/h2, cancellation, timeouts.
- [ ] M2.3 Request building: all body modes (multipart boundary, urlencoded, raw, binary, GraphQL JSON), automatic headers (no request-token header), header order and case, user headers win.
- [ ] M2.4 Response handling: gzip/deflate/br/zstd with decoded-size and ratio caps, max response size, size breakdown, streaming to file for Send and Download.
- [ ] M2.5 Redirects: manual loop, per-hop record, method rewrite rules, cross-host credential dropping, cap and loop detection.
- [ ] M2.6 Cookie jar (RFC 6265): persistence, `Set-Cookie` on every hop, per-request disable, allowlist storage.
- [ ] M2.7 Proxy: env vars, custom HTTP and SOCKS5, bypass list. Auth: Basic, Bearer, API key. Observation request 2.
- [ ] M2.8 Send pipeline in `engine` (12 steps, scripts as no-op hooks), history and console models, runtime ownership, panic-to-error wrapping.
- [ ] M2.9 Tools: `request_send`, `request_cancel`, `response_*`, `cookies_*`, `history_*`, `console_*`, `variables_resolve`. End-to-end tests against testkit.

**Exit:** `postguy-cli run --script` sends to loopback: redirect loops, cookies, all compressions, self-signed TLS, slow and huge bodies, cancel; regression tests for compression bomb and redirect cap; send overhead budget measured.

#### M3 Desktop shell and HTTP UI (ten sub-steps; commit per sub-step)

- [ ] M3.1 App shell (eframe, glow), theme tokens (light/dark/system), icon pipeline, widgets: square checkbox, outlined radio, switch, select with groups and checks, split button, popup list, context menu, modal and typed-confirm dialogs, inline rename, key-value table, field with `{{var}}` chips, code editor, large-text view, timeline skeleton. kittest tests for each.
- [ ] M3.2 Sidebar: collections tree (carets, preview-open, keyboard, multi-select, drag and drop, context menus exactly as PROMPT.md 5.1), environments, history, filter.
- [ ] M3.3 Tab strip: preview tabs, dirty dot, drag reorder, middle-click, overflow search, context menu, closed-tab reopen, session and draft restore, crash recovery.
- [ ] M3.4 Request builder: breadcrumb, URL field (chips, autocomplete, cURL paste), Params and path variables, Headers (auto-generated rows, presets, bulk edit), Body (all modes, Beautify), Authorization (No Auth, Inherit, Basic, Bearer, API Key), Settings, Docs, Save flows, send/cancel states.
- [ ] M3.5 Response viewer: header strip with popovers, Pretty/Raw/Preview/Hex, large-body strategy, search, headers/cookies tabs, Save Response, Send and Download. Frame-time and memory checks against section 8.
- [ ] M3.6 Environment editor, selector, quick-look, Globals; variable hover popover and "Set as variable".
- [ ] M3.7 Collection and folder detail tabs (Overview, Authorization, Variables, Scripts shell, Runs shell), inheritance display.
- [ ] M3.8 Command registry, command palette, trash view, tree undo.
- [ ] M3.9 Console panel, cookies manager, settings tab, bottom bar, right-side code snippet panel (cURL, fetch, requests), two-pane layout.
- [ ] M3.10 Shortcuts table, `--control` channel, `postguy-cli ui`, headless `shot` example, `xtask screenshots`. Observation request 3 reconciled into PROMPT.md 6.5 and 7.

**Exit:** layout matches PROMPT.md section 6; budgets measured (cold start, idle CPU, 10 MB response, 10 000-request tree); kittest coverage for tree, tabs, dirty tracking, selector, tables, editors; screenshots reviewed. **REVIEW STOP 2 (owner).**

#### M4 Imports, snippets, exports

- [ ] M4.1 Import dialog (paste, files, folders, drop anywhere), auto-detect, report view, name-clash handling.
- [ ] M4.2 Snippets: raw HTTP, JS fetch, Python requests, Go, Rust reqwest, PowerShell, Node axios; golden tests per language.
- [ ] M4.3 Export dialogs (collection v2.1 and native, environment, globals, response body), all-data zip, zip import.
- [ ] M4.4 Parity file, docs and screenshots up to date; budget report.

**Exit: tag v0.1. REVIEW STOP 3 (owner).**

### Phase 2: v0.2 (M5-M7)

#### M5 Remaining HTTP auth, certificates, GraphQL

- [ ] M5.1 `auth`: Digest (challenge retry once), JWT (HS/RS/ES/PS), OAuth 1.0, AWS Signature v4, Hawk. Known-answer tests from the specs; `curl` or external tools only as test oracles.
- [ ] M5.2 OAuth 2.0 in `http`: all grants, PKCE, loopback redirect listener opened only on demand and closed after, token store (secrets file), Manage tokens, auto-refresh.
- [ ] M5.3 Client certificates (PEM and PFX), CA bundle UI, request Settings tab complete.
- [ ] M5.4 GraphQL body mode and basic introspection with a Refresh button.
- [ ] M5.5 UI for all auth forms (Appendix E) and tools. NTLM, EdgeGrid, ASAP only if time allows (P2).

#### M6 WebSocket

- [ ] M6.1 `ws` crate (tokio-tungstenite): handshake headers, subprotocols, TLS, redirects, ping/pong, close codes, reconnect policy, size caps. Testkit echo server.
- [ ] M6.2 Tools `ws_*` and end-to-end tests.
- [ ] M6.3 UI: request bar, composer (Text/JSON/XML/HTML/Binary with Base64 or Hex), saved messages, timeline (search, filters, expand, hexdump, time diff, ring buffer), settings. Observation request 4 (WebSocket part).

#### M7 gRPC

- [ ] M7.1 `grpc`: protox compile with import paths, descriptor pool, per-request service definitions in `protos/`.
- [ ] M7.2 Dynamic codec and unary invoke over tonic; TLS toggle, client certs, metadata, status and trailers; testkit tonic server from our own `echo.proto`.
- [ ] M7.3 Server, client and bidirectional streaming with Send, End streaming, Cancel; message caps.
- [ ] M7.4 Reflection (v1 then v1alpha), example generator, proto-aware completion data, `grpc-status-details-bin` decoding.
- [ ] M7.5 Tools `grpc_*` and end-to-end tests for every streaming mode.
- [ ] M7.6 UI: request bar, method selector, tabs, service definition tab, message editor with autocomplete and type tooltips, timeline, settings, saved messages, examples, grpcurl snippet. Observation request 4 (gRPC part).

**Exit: tag v0.2. REVIEW STOP 4 (owner).**

### Phase 3: v0.3 (M8-M9)

#### M8 Scripts and tests

- [ ] M8.1 `script`: boa context, limits (wall clock, loops, recursion, memory), cancellation, `catch_unwind`.
- [ ] M8.2 `pm` API in Rust (variables, request, response, `pm.test`, Chai-compatible `expect`, `pm.info`, `pm.execution.skipRequest`, console, limited `require`). Unsupported APIs fail with clear messages.
- [ ] M8.3 Pipeline steps 2 and 11 wired in; test results model; gRPC hooks.
- [ ] M8.4 UI: script editors with autocomplete and snippets, Test Results tab, script error banner, Console output.
- [ ] M8.5 Sandbox fuzzing and hostile-script regression tests (infinite loop, memory blowup, deep recursion).

#### M9 Runner, CLI, remaining importers, MCP

- [ ] M9.1 `runner`: ordering, iterations, delay, data files (CSV and JSON), stop-on-error, cookie options, results model.
- [ ] M9.2 Runner tab (config, results, history under the collection Runs tab).
- [ ] M9.3 `postguy-cli run` with cli, json, junit, html reporters and the option set in PROMPT.md 5.6.3; exit codes.
- [ ] M9.4 OpenAPI 3.x and Swagger 2.0 (JSON and YAML) importer with option dialog; HAR importer; `.proto` import dialog.
- [ ] M9.5 MCP server complete (opt-in), tool table reconciled with the UI, parity file brought to date.

**Exit: tag v0.3. REVIEW STOP 5 (owner).**

### Phase 4: v1.0 (M10)

- [ ] M10.1 Budgets enforced in CI (`xtask size`, `xtask bench`), duplicate-dependency check, regression margins.
- [ ] M10.2 Accessibility pass (keyboard operability, contrast 4.5:1, accesskit), UI zoom, shortcuts customisation if time.
- [ ] M10.3 Packaging: macOS universal `.app` and dmg, Windows MSI and portable zip, Linux tar.gz/deb/AppImage, FreeBSD pkg; original "P" app icon in all sizes.
- [ ] M10.4 Release workflow, user docs (screenshots from our own UI), licence list (`xtask licenses`), security review, final gap list in `docs/gaps.md`.

**Exit: tag v1.0. REVIEW STOP 6 (owner).**

---

## 4. Dependencies between phases

M0 gates everything. M1 gates M2 (model, variables, store). M2 gates M3 (engine). M3 widgets are reused by M5-M9 UIs. M6 and M7 need the M2 connector and testkit. M8 needs M2 and the M3 editors. M9 needs M8 (scripts) and M1 (importers). M5 can run in parallel with M6 once M3 is done, but the plan keeps them sequential to limit review surface.

## 5. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Scope is very large | Strict tiers. Ship v0.1 before any P2. Cut P2 work first if time runs short |
| Heavy UI parts (key-value tables, code editor with autocomplete, virtualised viewers) | Build once in M3.1 with kittest tests; reuse for WebSocket, gRPC, scripts |
| Dependency licences (`ring`, `protox`, `boa`, others) outside the allowlist | `xtask deny` from M0; any non-allowlisted licence goes to the owner first |
| Unverified Postman details | Observation requests (section 2); keep unverified items marked until confirmed |
| Idle CPU and binary size drift | `xtask size` and `xtask bench` at the end of every milestone |
| `panic = "unwind"` costs size | Accepted. Recover with `default-features = false`, LTO, no `regex`, `chrono`, `syntect`, `resvg` |
| hyper-direct takes too long | Spike decides; reqwest is the documented fallback |
| boa too slow or incomplete | Spike decides; `rquickjs` needs owner approval |
| Hostile input crashes | Fuzzing from M1; catch_unwind guards at import, script and command entry points; every fix has a failing-first test |
| Tracked state goes stale | Update STATUS, this file, ROADMAP and parity at the end of each session |

## 6. Definition of done for a feature

Engine API exists; automation tool exists with an end-to-end test; UI exists and was inspected through the control channel or a screenshot; unit/property tests added; no new panic path; budgets still met; `parity/postman-features.toml` entry updated with evidence and honest notes; docs touched where behaviour changed.

## 7. Immediate next steps

1. ~~Owner answers the two open questions in section 0 (tool installs, moving `prompt_images/`).~~ Defaults applied per section 0 (see the resolution note); the tool installs stay open for the owner.
2. ~~Start M0.1.~~ M0.1-M0.5 done (2026-10-19). **REVIEW STOP 1: owner reviews the skeleton, the ADRs (`plan/adr/0001-0005`) and the open script-engine decision before M1 starts.**
3. Owner's Postman observation request 1 (PLAN.md §2) is next on the owner's side; M1.6 consumes it.
