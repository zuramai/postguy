# postguy-interop

- **Layer:** L2 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** skeleton, planned for M1.6

Import/export: Postman v2.0/v2.1, environments and globals, cURL, later OpenAPI/Swagger and HAR; ImportReport; bounded input sizes and nesting.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
