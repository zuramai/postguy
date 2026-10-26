# postguy-vars

- **Layer:** L1 (checked by `cargo xtask layers`)

Variable scopes and precedence, {{var}} resolver with depth limit and cycle guard, dynamic variables, secret masking; pure.

- Scope precedence, lowest first: globals, collection, folder, environment, data, runtime, local.
  The runtime scope is session-lived (set by `bru.setVar`/`pg.setVar`) and never written to disk.
- `shortcode` — body shortcodes `{{% base64(file('lib/a.jpg')) %}}`: `call := ident(arg, ...)`,
  `arg := call | 'string' | "string"`; functions `file`, `text`, `base64`, `base64url`, `hex`,
  `sha256`, `md5`; limits: depth 8, 4,096 bytes per shortcode, 1,000 per text. `scan` parses
  only (for the UI), `expand` evaluates with a caller-supplied asset loader and argument resolver
  (so `{{var}}` works inside arguments), `single_file` detects a lone `file('p')`. The `{{var}}`
  resolver skips `{{%` so shortcodes pass through it untouched.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
