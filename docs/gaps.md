# Gaps (tracked, ranked)

**Status:** M0. Gaps are ranked by user-visible importance; prefer them over new P2/P3 features.
Each row names the evidence that would close it.

## Open (ranked)

1. **Script engine memory/wall-clock limits** — boa 0.22 exposes none (measured, ADR 0004): a
   hostile script's allocation bomb can OOM-kill the app. Owner decision pending: (a) ship
   containment or (b) approve rquickjs. Evidence to close: an owner decision recorded in the
   ADR, then the chosen engine passes M8.5's hostile-script regression tests (infinite loop,
   memory blowup, deep recursion).
2. **Header case on the wire** — hyper writes header names lowercase (http::HeaderName); user
   typed case is not preserved. Confirm whether that differs observably from Postman via
   observation request 2 (echo server headers); close by documenting the behaviour
   (docs/cli or scripting docs) or implementing a raw serializer if the owner decides it
   matters.
3. **Idle memory on the real shell** — spikes measured 92-105 MiB max RSS for a demo window;
   the 90 MiB empty-workspace budget is measured at M3 with the actual app (then enforced).
   Evidence: `xtask size`/bench report at M3 end.
4. **`xtask bench`** — the timing harness (cold start with 10 tabs, tree of 10 000 requests,
   10 MB Pretty) arrives with the shell (M3.5/M3 exit). Now only the spike numbers exist.
5. **cargo-deny advisories** — licence/source checks run locally; `RUSTSEC` advisories need
   cargo-deny installed or CI's cargo-deny-action (runs in CI now). Close: owner approves the
   install; `xtask deny` then delegates.
6. **Fuzz targets** — driver exists, registry empty; first importer/parser targets in M1.9.
   Close: `cargo xtask fuzz` runs targets clean for 5 minutes.
7. **Observation requests 1-3** — Postman defaults (auto headers, settings defaults, shortcuts)
   unverified until the owner runs them with synthetic data. Items depending on them keep
   their parity notes unverified.

## Closed in M0

- Renderer choice (ADR 0001: glow), HTTP stack choice (ADR 0003: hyper-direct), large body
  strategy (ADR 0002), icon/logo pipeline (ADR 0005).
