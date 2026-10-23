# Release packaging

`.github/workflows/release.yml` runs on pushes to the `release` branch (or by hand with an
optional version override) and calls the shared `storytold/craft-common-actions` release workflow,
which runs the scripts in this folder:

| Platform | Script | Artifacts in `dist/release/` |
| --- | --- | --- |
| macOS universal | `macos/package.sh --arch universal` | `.dmg`, `.zip` (app), CLI `.tar.gz`; Developer ID codesign and notarization |
| Windows x64, x86, arm64 | `windows/package.ps1 -Arch <arch>` | WiX v5 `.msi`, portable `.zip`, CLI `.zip`; Azure Trusted Signing (or PFX) |
| Linux x86_64, aarch64 | `linux/package.sh` | `.AppImage`, `.deb`, `.rpm`, `.tar.gz`, CLI `.tar.gz` (nfpm) |
| Flatpak | `linux/flatpak-bundle.sh` | `.flatpak` (the shared workflow always runs it) |

Not built: WebAssembly (AGENTS.md: there is no web build), riscv64, FreeBSD and Arch.

`env.sh` is shared setup (version from the root `Cargo.toml`, `dist/release`, application id
`io.github.zuramai.postguy`). Version overrides use `cargo xtask version set X.Y.Z`.

Signing secrets (set in the repository's `release` environment): `APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`;
`AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`,
`AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE` (or `WINDOWS_CERTIFICATE` and
`WINDOWS_CERTIFICATE_PASSWORD`). Outside the `release` branch the scripts warn and ship unsigned.

Icons are derived at packaging time from `assets/logo`; no generated icon is committed.
