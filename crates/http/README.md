# postguy-http

- **Layer:** L3 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** skeleton, planned for M2.2

HTTP client: connect/TLS/timing, redirects (manual loop with hop records), compression
(gzip/deflate/br/zstd with decoded-size caps), an RFC 6265 cookie jar, proxy, auth retries,
streaming bodies with size cap, cancellation.

- `client` — one connection per request (pooling comes with its own measurement), timing
  breakdown, body caps, protocol negotiation.
- `decode` — `Accept-Encoding` negotiation and a bounded streaming decoder.
- `redirect` — the manual follow loop: 301/302/303/307/308 rules, per-hop records, cross-host
  credential dropping, hop cap and loop detection. `send_with_cookies` also captures
  `Set-Cookie` on every hop and merges jar cookies with a manual Cookie header.
- `proxy` — route resolution from `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY` or the
  custom Settings > Proxy block (custom wins), absolute-form plain HTTP through an HTTP proxy,
  CONNECT tunnels for HTTPS, SOCKS5 and SOCKS5h (local or proxy DNS) handshakes, optional proxy
  basic auth, wildcard bypass lists. Credentials never print (`Debug` redacts them).
- `auth` — the v1 P0 set applied to a prepared spec: Basic, Bearer (optional prefix), API key
  in a header or the query; other kinds fail with an actionable "not supported yet" error.
- `jar` — the visible, persistent cookie jar (`stored` in `.postguy/cookies.json` by the
  store): domain/path/secure/expiry rules, per-domain and total caps, domain allowlist for
  scripts. No public-suffix list yet (documented gap); SameSite stored, not enforced.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
