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

- **2026-10-19 (M0):** Session start. Repo was empty (AGENTS.md, PROMPT.md, PLAN.md,
  `prompt_images/`); toolchain rustc/cargo 1.98 nightly, macOS arm64. `cargo-deny` and
  `cargo-bloat` are not installed, so per PLAN §0 the `xtask deny` and `xtask size` checks are
  implemented by xtask itself (deny.toml is still shipped for CI and for when the tools are
  approved). `prompt_images/` moved to `plan/postman/` as PLAN M0.1 specifies. Started M0.1
  (repo hygiene) and M0.2 (workspace skeleton).
