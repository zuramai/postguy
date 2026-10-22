# Development

## Toolchain

Rust 1.98 nightly on this machine; the workspace pins `rust-version = "1.95"` (what egui 0.36
requires). No font files, no `craft-*` dependencies, no WASM target.

## Everyday commands

```
cargo build -p postguy                    # desktop app
cargo run -p postguy -- --workspace DIR --control /tmp/postguy-control.json
                                          # opt-in control channel (loopback, fresh owner-only token)
cargo run -p postguy-cli -- run --script STEPS.json --root DIR   # engine harness
cargo run -p postguy-cli -- ui --control FILE inspect             # or command/set/screenshot
cargo run -p postguy-cli -- call import_file --json '{"path":"collection.json"}'
cargo run -p postguy-cli -- call snippet_generate --json '{"collection":"C","request":"R","language":"rust"}'
cargo xtask ci                            # fmt, clippy -D warnings, test, layers, assets, parity, deny
cargo xtask layers                        # dependency layering only
cargo xtask assets [--write]              # asset policy against ATTRIBUTION.toml (ATTRIBUTION.md regenerate)
cargo xtask parity [--partial|--json]     # parity checklist validation and progress
cargo xtask deny                          # licences/sources (delegates to cargo-deny when installed)
cargo xtask size [--skip-build]           # release binary sizes against the size budgets
cargo xtask bench [--check] [--debug]      # headless frame-time capture (10k tree, 10 MB Raw)
cargo xtask icons [--check]               # regenerate ui-egui/src/icons_gen.rs from assets/icons
cargo xtask logo [--check]                # rasterize assets/logo/postguy.svg into PNG sizes
cargo xtask fuzz --time 300 [--seed N]    # mutation fuzzing (registered targets)
cargo xtask screenshots [--out DIR]       # render review screenshots into target/screenshots/
```

## Working rules (short form; AGENTS.md is the law)

- Conventional commits (`feat(http): follow redirects with cookie capture`), branch
  `<type>/<slug>`, commit only green states; main is integration.
- Scope builds/tests to the crates touched (`-p`, `-j 2`, under a timeout); the full sweep is
  CI's job (`cargo xtask ci`) and CI runs it on every PR.
- Every feature: engine API → automation tool (with an end-to-end test) → UI; a feature without
  its tool is not done (crates/automation README, "Adding a tool").
- Tests are hermetic: loopback servers in `postguy-testkit`; the internet is not touched by
  tests. Fuzz findings become synthetic regression tests seen failing first.
- Update the parity row of the feature you touched.
- UI verification: run `cargo run -p postguy-ui-egui --example shot -- OUT.png dark` (or `cargo xtask screenshots`), or launch the app with `--control` and use `postguy-cli ui`; `click`/`key`/`type` are not emulated (docs/gaps.md).

## Gates the CI bundle runs

`cargo xtask ci` = fmt --check → clippy --workspace --all-targets -D warnings → test
--workspace → layers → assets → parity → deny. clippy.toml allows unwrap/expect/panic in
tests and forbids egui's `checkbox`/`radio` draw paths in favour of our widgets.

## Parallel agents

Separate `CARGO_TARGET_DIR` and separate git worktrees; ad-hoc spike crates opt out of the
workspace with an empty `[workspace]` table in their manifest.
