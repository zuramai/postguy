# postguy-snippets

- **Layer:** L3 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** skeleton, planned for M4.2

Code generation for cURL, raw HTTP, fetch, requests, Go net/http, reqwest, PowerShell, axios, grpcurl; pure.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
