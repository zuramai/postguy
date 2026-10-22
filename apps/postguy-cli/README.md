# postguy-cli

The headless CLI. Subcommands:

- `tools` / `call` / `run --script`: the automation tool table over the engine; `--root DIR`
  confines every path.
- `ui`: a client for the app's opt-in control channel (`inspect`, `command`, `set`,
  `screenshot`). `click`, `key` and `type` are refused by the app, which does not emulate
  synthetic input.
- `fuzz-target`: dev verb used by `cargo xtask fuzz`.

The collection runner with reporters and the `mcp` server are not implemented yet.
