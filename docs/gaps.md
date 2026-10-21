# Gaps (tracked, ranked)

**Status:** M4.3 (imports, snippets and export dialogs in; response-body picker and full zip restore still open).
Gaps are ranked by user-visible importance; prefer them over new P2/P3 features. Each row names
the evidence that would close it.

## Open (ranked)

1. **Control-channel synthetic input** — `--control` implements inspect/command/set/screenshot,
   but `click`, `key` and `type` return a refusal: safe eframe/egui has no supported way to
   inject raw pointer/key events into a running app pass. Closing it needs either a
   kittest-driven `control.rs` suite over the shell (the honest alternative) or an owner
   decision on a platform input-injection dependency.
2. **URL-to-file import** — PROMPT 5.7 asks the import box to accept a URL and fetch it only on an explicit user action; M4.1 refuses with guidance instead (no implicit network). Close with an explicit Fetch button using the engine's capped GET, showing the size and format before importing.
3. **Zip restore is documents-only** — all-data export includes files/protos/history, but import reads .json documents and skips the rest. Close by staging entries with path validation and merging files/protos with the same clash rules.
4. **Response body save picker** — Save Response writes `<workspace>/files/response-<tab>.bin`; the M4.3 dialog/name suggestion is still missing.
5. **Idle RSS over budget** — the release app measured 100.7 MiB RSS on an empty workspace at
   M3 exit (budget 90 MiB; idle CPU 0.2% and cold start ~176 ms first inspect are inside
   budget). Close by profiling the eframe/glow/engine baseline and trimming loaded state or
   fonts; evidence: `ps -o rss` on a release build after 3 s idle.
6. **10 MB Pretty is disabled** — `PRETTY_CAP` is 5 MB and Pretty formats on the UI thread when
   under it, while the budget asks for a 10 MB Pretty view in <= 1.5 s off-thread. Raw is
   virtualised and measured (first frame 10 ms). Close with off-thread formatting, a
   formatting indicator, and a raised cap; evidence: the `response.pretty_ms` bench metric.
7. **Console virtualisation** — the engine ring buffer masks secrets and caps at 5000 entries,
   but the panel lays out every filtered row in one ScrollArea. Close with `show_rows` and a
   5000-row frame-time test.
8. **Script engine memory/wall-clock limits** — boa 0.22 exposes none (measured, ADR 0004): a
   hostile script's allocation bomb can OOM-kill the app. Owner decision pending: (a) ship
   containment or (b) approve rquickjs. Evidence to close: an owner decision recorded in the
   ADR, then the chosen engine passes M8.5's hostile-script regression tests (infinite loop,
   memory blowup, deep recursion).
9. **Header case on the wire** — hyper writes header names lowercase (http::HeaderName); user
   typed case is not preserved. Confirm whether that differs observably from Postman via
   observation request 2 (echo server headers); close by documenting the behaviour
   (docs/cli or scripting docs) or implementing a raw serializer if the owner decides it
   matters.
10. **Cold start with 10 restored tabs not measured** — empty-workspace first inspect is
   ~176 ms; the budget's 10-tab session needs a synthetic session fixture and the same
   measurement.
11. **Remaining M3 budget metrics** — send overhead (click to first byte, last byte to Pretty
   for 100 KB), 100 MB body, 50 000 timeline messages and Pretty for 10 MB are not in
   `xtask bench` yet.
12. **cargo-deny advisories** — licence/source checks run locally; `RUSTSEC` advisories need
   cargo-deny installed or CI's cargo-deny-action (runs in CI now). Close: owner approves the
   install; `xtask deny` then delegates.
13. **Fuzz coverage** — `url`, `vars` and `curl` targets run; the Postman/OpenAPI/HAR
    importers, cookie parser and script sandbox still need targets. Close: `cargo xtask fuzz
    --time 300` runs the full registry clean.
14. **Observation requests 1-3** — Postman defaults (auto headers, settings defaults,
    shortcuts) unverified until the owner runs them with synthetic data. Items depending on
    them keep their parity notes unverified.

Additional M3 leftovers, ranked below the above: cookies manager add/edit and templates; the
Save Response example picker and 5 MB example cap; splitter position/two-pane persistence in
the session; shortcut-conflict checking; history day grouping.

## Postman v12 feature investigation matrix

Every area the Postman v12 documentation index lists (`https://learning.postman.com/llms.txt`,
read 2026-02) mapped to what Postguy has. Statuses use the parity vocabulary: `shipped` =
usable today with a cited test, `partial` = part exists (the milestone says what is missing),
`planned` = not started, `na` = deliberately out of scope for an offline app. The authoritative
per-feature record is `parity/postman-features.toml`; this table is the reading order for
someone deciding what to build next.

| Postman v12 doc area | Postguy status | Where it stands |
|---|---|---|
| Get started: workbench, install, quick start, system requirements | **partial** | `h.window`, `h.tabs`, `h.sidebar` are shipped; local workbench is complete. Sign-up/sign-in and cloud onboarding are `na` (no account, no network without a user action) |
| Send requests: request builder, params, headers, body modes | **shipped** | `c.body-modes`, `c.params-tab`, `c.headers-tab`; per-request settings shipped (`c.settings-tab`) |
| API authentication and authorization | **partial** | Basic/Bearer/API key/Digest/JWT/OAuth 1.0/OAuth 2.0/AWS Sig v4/Hawk applied (`c.auth-p0`, `c.auth-m5`). Missing: implicit grant and the Manage-tokens screen (M5) |
| Variables, environments, globals, dynamic values | **shipped** | `b.environments-crud`, `b.globals`, `b.resolution`, `b.dynamic-variables`, `b.secrets`. Remaining polish is `b.chips-and-autocomplete`'s popover actions and `b.set-as-variable` |
| Response data and the response viewer | **partial** | `c.response-viewer`: Pretty/Raw/Preview/Hex, syntax colours, search tint, save/copy. Open: image previews, regex/whole-word search |
| Debug API requests (console, troubleshooting) | **partial** | `h.console` (filters, masking, ring buffer). Open: virtualised scrolling, request/response body inspector |
| GraphQL requests | **shipped** | `c.body-modes`: query/variables/operation editors, schema introspection with Refresh |
| gRPC requests | **partial** | `d.unary-invoke`, `d.reflection`, `d.streaming` work through descriptors; the grouped method picker, metadata table, proto-aware editor, trailers and grpcurl snippet are M7 |
| WebSocket requests | **partial** | `e.connect`, `e.subprotocols`, `e.timeline`; binary composers, saved messages and reconnect policy are M6.3 |
| MCP requests (Postman as an MCP *client*) | **na** | Not in the brief. Postguy ships the opposite direction: an opt-in MCP *server* (`f.mcp`, M9.5) so agents can drive it |
| AI requests / Agent Mode / Postbot | **na** | Offline by design: no model endpoint, no telemetry. The local equivalent is the `automation` tool table an agent already drives |
| Collections and folders | **shipped** | `a.tree-view`, `a.create-organise`, `a.keyboard-and-multiselect`, `a.filter`, `a.collection-auth`, `a.overview-tab` |
| Native Git | **na** | A Postguy workspace is plain files in a folder, so any Git client versions it; a built-in Git UI is not planned |
| Postman Vault (cloud secrets) | **na** | Local equivalent: `.postguy/secrets.json` at 0600 with masking (`b.secrets`) |
| Tests, scripts and assertions | **planned (M8)** | `f.scripts`, `f.expect`, `f.test-results`, `f.script-ui`, `f.sandbox-hardening`. Until then scripts are an explicit no-op hook, and History/Console record that |
| Collection Runner and data-driven runs | **planned (M9.2)** | `f.runner`; deterministic order, iterations, delay, stop-on-error. Performance/load testing (Postman's cloud runners) is `na` |
| Postman CLI / Newman: run collections from CI | **planned (M9.3)** | `f.runner-cli` with cli/json/junit/html reporters and exit codes; `f.run-script` already drives the engine |
| Import (collections, environments, specs, other clients) | **partial** | `g.postman-collection`, `g.postman-env`, `g.curl`, `g.import-dialog` shipped. OpenAPI/Swagger, HAR and `.proto` importers are M9.4 (`g.openapi`, `g.har`, `g.proto-import`) |
| Export and all-data backup | **partial** | Postman/native export and snippets shipped; `g.all-data-zip` exports but document-only restore is open (gap 3); a response-body picker is open (gap 4) |
| Code snippets (cURL, HTTP, fetch, requests, Go, reqwest, PowerShell, axios) | **shipped** | `g.snippets`, syntax-coloured in the right panel; grpcurl is M7.6 |
| Mock servers and API design (Spec Hub) | **na** | Running a mock server contradicts "no listener by default"; OpenAPI *import* (M9.4) is the useful slice and is planned |
| Monitors, scheduled runs | **na** | Needs a scheduler and cloud execution; the CLI runner (M9.3) is the local equivalent |
| Flows (low-code editor) | **na** | Out of scope by design |
| API governance, scorecards, catalog | **na** | Out of scope by design |
| Documentation and publishing (including "Run in Postman") | **na** | The local description/Docs tab is shipped (`h.detail-tabs`, `h.builder`); publishing to a public domain needs a service |
| SDK and CLI generators | **na** | Out of scope by design |
| Postman API, VS Code extension, plugin marketplace | **na** | The local equivalents are `postguy-cli` (`f.agent-tools`, `f.run-script`), the opt-in MCP server (`f.mcp`) and the opt-in UI control channel (`f.ui-control`) |
| Collaboration, workspaces sharing, version control, webhooks, integrations, Insights | **na** | Offline and single-user by design; sharing is "copy the folder" |
| Administration, billing, plans, SSO/SCIM, API keys | **na** | No accounts |
| Platform: packaging, budgets, accessibility | **partial** | `i.platform-services` and `i.packaging` are open (M10); byte/bench budgets are enforced but idle RSS is over budget (gap 5) |

Two observations from the audit worth keeping in view: the highest-value missing *workbench*
features are the script sandbox (M8) and the runner (M9), because everything Postman does with
collections as test suites depends on them; and almost every `na` above is a cloud capability,
so the offline surface is now short only of M5 auth polish, M6-M7 protocol polish and M8-M9
automation.

## Closed in M3

- 10 000-request tree and 10 MB Raw frame-time harness (`xtask bench`, measured 5.8/10.2 ms on
  the M3.10 machine).
- One minified 10 MB line froze the viewer (a single 10 MB galley); `LargeTextView` now splits
  physical long lines into <= 4 KiB display rows and caches the Raw text per buffer.
- Shortcut table, control channel (inspect/command/set/screenshot), `postguy-cli ui`, headless
  `shot` example and `xtask screenshots`.

## Closed in M0

- Renderer choice (ADR 0001: glow), HTTP stack choice (ADR 0003: hyper-direct), large body
  strategy (ADR 0002), icon/logo pipeline (ADR 0005).
