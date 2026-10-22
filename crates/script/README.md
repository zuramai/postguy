# postguy-script

- **Layer:** L3 (enforced by `cargo xtask layers`)
- **Status:** not implemented yet; the crate is an empty placeholder.

Embedded JS sandbox (boa_engine) with the pm API, wall-clock and memory limits, cancellation; behind the scripting feature.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
