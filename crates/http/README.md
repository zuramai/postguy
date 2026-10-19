# postguy-http

- **Layer:** L3 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** skeleton, planned for M2.2

HTTP client: connect/TLS/timing, redirects, compression, cookie jar, proxy, auth retries, streaming bodies with size cap, cancellation.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
