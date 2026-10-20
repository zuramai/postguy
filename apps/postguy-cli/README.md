# postguy-cli

The headless CLI. Subcommands (PROMPT.md 5.6.3): `run` (collection runner with reporters),
`tools`/`call`/`run --script` (automation tools; `--root` confines every path), `import`/`export`,
`snippet`, and the opt-in `mcp` and `ui` subcommands. It is the same tool table the MCP server
serves. Status: the automation table and `run --script` are implemented; `ui` implements the control client (`inspect`, `command`, `set`, `screenshot`; `click`/`key`/`type` are refused by the app, which does not emulate synthetic input). `run` with reporters and `mcp` are later milestones.
