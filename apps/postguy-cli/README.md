# postguy-cli

The headless CLI. Subcommands:

- `tools` / `call` / `run --script`: the automation tool table over the engine; `--root DIR`
  confines every path.
- `run --collection ID_OR_NAME --workspace DIR`: the collection runner (`--folder`, `--environment`,
  `--iterations`, `--delay`, `--data`, `--bail`, `--requests`) with `--reporters
  cli,json,junit,html,csv,xlsx,pdf` and `--out DIR`; files are `postguy-run-<run id>.<ext>` (`xml`
  for junit). Exit 0 all passed, 1 failures, 2 usage error.
- `ui`: a client for the app's opt-in control channel (`inspect`, `command`, `set`,
  `screenshot`). `click`, `key` and `type` are refused by the app, which does not emulate
  synthetic input.
- `fuzz-target`: dev verb used by `cargo xtask fuzz`.

The `mcp` server is not implemented yet.
