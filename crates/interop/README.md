# postguy-interop

- **Layer:** L2 (enforced by `cargo xtask layers`)

Import/export: Postman v2.0/v2.1, environments and globals, cURL, with an ImportReport; OpenAPI/Swagger and HAR are not implemented yet; bounded input sizes and nesting.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
