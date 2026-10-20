# postguy-ui-egui

- **Layer:** L7 (PROMPT.md 4.1; enforced by `cargo xtask layers`)
- **Status:** theme, icon renderer, widget kit and app shell (M3.1); screens follow in M3.2-M3.9

The egui shell: theme, widgets and all screens.

- `theme` — light/dark semantic tokens (every text token >= 4.5:1, asserted), System/Light/Dark
  preference through egui's OS signal, spacing and type scale.
- `icons` — a small SVG path-data parser (M/L/H/V/C/S/Q/T/A/Z) that flattens the generated
  Lucide data into stroked polylines; no SVG renderer and no icon font ships.
- `widgets` — custom controls (square checkbox, outlined radio, switch, grouped/searchable
  select, split button, context menu rows, modal, typed confirm, inline rename), the
  KeyValueTable, the `{{var}}` field, CodeEditor, LargeTextView and Timeline. Every control
  reports AccessKit state and has a kittest.
- `shell` — header, sidebar, tab-strip placeholder, central panel, optional right panel and
  bottom bar; `apps/postguy` hosts it with eframe/glow.
- `examples/shot.rs` — headless wgpu render of the real shell (`cargo run -p postguy-ui-egui
  --example shot -- out.png light|dark`), the start of the M3.10 shot harness.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
