# Gaps (tracked, ranked)

**Status:** M3.10 (shell feature-complete; M3 exit measurements recorded, REVIEW STOP 2 pending).
Gaps are ranked by user-visible importance; prefer them over new P2/P3 features. Each row names
the evidence that would close it.

## Open (ranked)

1. **Control-channel synthetic input** — `--control` implements inspect/command/set/screenshot,
   but `click`, `key` and `type` return a refusal: safe eframe/egui has no supported way to
   inject raw pointer/key events into a running app pass. Closing it needs either a
   kittest-driven `control.rs` suite over the shell (the honest alternative) or an owner
   decision on a platform input-injection dependency.
2. **Idle RSS over budget** — the release app measured 100.7 MiB RSS on an empty workspace at
   M3 exit (budget 90 MiB; idle CPU 0.2% and cold start ~176 ms first inspect are inside
   budget). Close by profiling the eframe/glow/engine baseline and trimming loaded state or
   fonts; evidence: `ps -o rss` on a release build after 3 s idle.
3. **10 MB Pretty is disabled** — `PRETTY_CAP` is 5 MB and Pretty formats on the UI thread when
   under it, while the budget asks for a 10 MB Pretty view in <= 1.5 s off-thread. Raw is
   virtualised and measured (first frame 10 ms). Close with off-thread formatting, a
   formatting indicator, and a raised cap; evidence: the `response.pretty_ms` bench metric.
4. **Console virtualisation** — the engine ring buffer masks secrets and caps at 5000 entries,
   but the panel lays out every filtered row in one ScrollArea. Close with `show_rows` and a
   5000-row frame-time test.
5. **Script engine memory/wall-clock limits** — boa 0.22 exposes none (measured, ADR 0004): a
   hostile script's allocation bomb can OOM-kill the app. Owner decision pending: (a) ship
   containment or (b) approve rquickjs. Evidence to close: an owner decision recorded in the
   ADR, then the chosen engine passes M8.5's hostile-script regression tests (infinite loop,
   memory blowup, deep recursion).
6. **Header case on the wire** — hyper writes header names lowercase (http::HeaderName); user
   typed case is not preserved. Confirm whether that differs observably from Postman via
   observation request 2 (echo server headers); close by documenting the behaviour
   (docs/cli or scripting docs) or implementing a raw serializer if the owner decides it
   matters.
7. **Cold start with 10 restored tabs not measured** — empty-workspace first inspect is
   ~176 ms; the budget's 10-tab session needs a synthetic session fixture and the same
   measurement.
8. **Remaining M3 budget metrics** — send overhead (click to first byte, last byte to Pretty
   for 100 KB), 100 MB body, 50 000 timeline messages and Pretty for 10 MB are not in
   `xtask bench` yet.
9. **cargo-deny advisories** — licence/source checks run locally; `RUSTSEC` advisories need
   cargo-deny installed or CI's cargo-deny-action (runs in CI now). Close: owner approves the
   install; `xtask deny` then delegates.
10. **Fuzz coverage** — `url`, `vars` and `curl` targets run; the Postman/OpenAPI/HAR
    importers, cookie parser and script sandbox still need targets. Close: `cargo xtask fuzz
    --time 300` runs the full registry clean.
11. **Observation requests 1-3** — Postman defaults (auto headers, settings defaults,
    shortcuts) unverified until the owner runs them with synthetic data. Items depending on
    them keep their parity notes unverified.

Additional M3 leftovers, ranked below the above: cookies manager add/edit and templates; the
Save Response example picker and 5 MB example cap; splitter position/two-pane persistence in
the session; shortcut-conflict checking; history day grouping.

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
