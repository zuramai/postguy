# postguy-runner

- **Layer:** L4 (enforced by `cargo xtask layers`); pure, no I/O and no network.
- **Status:** run configuration, request plans, CSV/JSON data files, `setNextRequest`
  sequencing, the result model and the CLI/JSON/JUnit/HTML/CSV/XLSX/PDF reporters. The engine
  (`Engine::run_collection`) drives the sends and fills the result in.

| Module | What it holds |
| --- | --- |
| `config` | `RunConfig` and the iteration/delay caps (`MAX_ITERATIONS`, `MAX_DELAY_MS`). |
| `plan` | `plan()`: depth-first request order, folder scope, ordered subsets. gRPC and WebSocket requests stay in the plan with their protocol; the engine skips them. |
| `data` | `parse_data()`: CSV (header row) or JSON (array of objects), 10 MiB / 100 000 rows / 1 000 columns caps, 1-based row numbers in errors. |
| `sequence` | `Sequencer`: next request by `NextRequest`, a step cap against loops. |
| `result` | `RunResult`, `IterationResult`, `RequestRun` (with `started_ms`, custom `report` fields and a failed-response `response_excerpt`), `Totals`, `RunEvent`, `RunSummary`. |
| `report` | `cli()`, `json()`, `junit()`, `html()` (self-contained, no script, every string escaped), plus `table()` (the single column definition: summary, requests, tests, custom `pg.report` columns) feeding the hand-written `csv()` (BOM, CRLF, formula-injection guard), `xlsx()` (zip of inline-string sheets: Summary, Requests, Tests) and `pdf()` (PDF 1.4, landscape A4, non-embedded Courier, repeated header, page numbers). |

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
