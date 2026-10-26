# postguy-interop

- **Layer:** L2 (enforced by `cargo xtask layers`)

Import/export: Postman v2.0/v2.1, environments and globals, cURL, OpenAPI/Swagger, HAR and Bruno folders, with an ImportReport; bounded input sizes and nesting.

- `bruno` — `import_bruno(files, dotenv)` maps `bruno.json`, `.bru` requests, folder/collection
  `.bru` files and `environments/*.bru` to a collection, environments and a report (`lang` is the
  Bru parser: dict, text and list blocks, 5 MiB / 2,000 blocks caps). Request `assert` blocks become
  Tests-tab assertions, `tests` the test script, `vars:*` become `pg` script lines,
  `{{process.env.X}}` becomes `{{X}}` with a secret variable per environment, `@file(p)` becomes
  `{{% file('p') %}}`. Unsupported blocks, auth modes and npm `require`s are reported, not silently
  dropped. The function is pure; the engine walks the directory and copies non-`.bru` files into
  Assets. Behaviour comes only from the public docs at <https://docs.usebruno.com>; Bruno's
  source code is never consulted (clean room).
- Postman export writes non-empty request tests as the extra item key `postguyTests`, and import
  reads it back.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
