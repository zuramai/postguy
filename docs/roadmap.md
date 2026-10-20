# Roadmap

**Status:** M1-M4 complete through the M4.4 v0.1 feature set; **REVIEW STOP 3** pending. M5
(remaining auth) and v0.2 follow. The milestone table and exit criteria live in PLAN.md; this is
the milestone view only. The feature state lives in `parity/postman-features.toml`.

| Tag | Milestones | Content | Exit criteria (abridged) |
|---|---|---|---|
| — | M0 (done) | Skeleton, xtask gates, spikes + ADRs, tracking docs, logo | `cargo xtask ci` green; ADRs with numbers; window opens with the spike renderer |
| v0.1 | M1-M4 (M5 pending) | model/vars/urlkit/store/interop (Postman + cURL), automation tools (`workspace_*`…), HTTP engine, full desktop UI, imports/snippets/exports | Postman v2.1 round-trip goldens; UI budgets measured; kittest coverage; tag v0.1 after REVIEW STOP 3 |
| v0.2 | M6-M7 | WebSocket then gRPC (unary → streaming → reflection) | end-to-end tools + tests per protocol; tag v0.2 |
| v0.3 | M8-M9 | scripts (`pm` API), runner, CLI reporters, OpenAPI/HAR import, MCP | sandbox fuzzing green; reporters golden-tested; tag v0.3 |
| v1.0 | M10 | budgets enforced in CI, a11y pass, packaging (macOS/Windows/Linux/FreeBSD), docs | budgets met on record machines; security review; tag v1.0 |

M5 (remaining HTTP auth + client certs + GraphQL) ships inside v0.1 per PLAN.md Phase 1.

Open decisions listed in `plan/STATUS.md`; observation requests to the owner (Postman,
synthetic data only) sit in PLAN.md §2.

Milestone-end obligations: budgets measured (`xtask size`), parity entries updated honestly,
screenshots refreshed where UI changed, docs revised with a status line.
