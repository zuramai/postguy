# postguy-util

- **Layer:** L0 (checked by `cargo xtask layers`)

Small helpers: atomic path utilities, size-capped readers, depth-limited JSON walker, ring buffer, fractional ordering, time.

`time` formats local datetimes through `jiff`: `local_offset_seconds(ms)` (0 when the system zone is
unreadable, shown as `+00:00`) and `format_datetime(ms, offset, DateStyle::{Full, Iso, Compact}, now)`,
which is total (`"invalid time"` on out-of-range input).

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
