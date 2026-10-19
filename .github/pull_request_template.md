<!--
One task id per commit (e.g. `M2.3: redirect loop with cookie capture`). CI must be green
before merge; a reviewer asks for provenance first, then safety, then completeness.
-->

## What

<!-- What changes, in one para. Link the task id (PLAN.md) and any parity entry it affects. -->

## Context

<!-- Why: the problem, the observed behaviour, the Postman feature it matches (PROMPT.md section), the ADR if a pattern changed. -->

## Changes

<!-- Bulleted list of the real changes: crates touched, public API deltas, migration notes. Nothing oblique. -->

## Rules check (AGENTS.md)

- Provenance: no Postman-authored assets; no code ported from other implementations; no font files committed.
- Safety: no new panic path or `unsafe`; every input-sized value checked/capped; last-resort guards where untrusted input runs.
- Automation: the feature is reachable headlessly (tool in `crates/automation`) with an end-to-end test.
- Parity: `parity/postman-features.toml` updated honestly (`shipped` only with cited tests).
- Layering/budgets: `cargo xtask layers` clean; no repaint loop; no UI-thread I/O; release profile untouched.

## Evidence

<!-- The exact commands you ran and their results (local, scoped `-p` builds): fmt, clippy, tests, xtask gates. Paste the tail of the run. -->

## Screenshots

<!-- For UI changes: from `postguy --control FILE` + `postguy-cli ui screenshot`, or the `shot` example. Demo data only. -->
