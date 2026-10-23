# Release packaging

`.github/workflows/release.yml` runs when a `v*` tag is pushed (the tag names the version) or by hand with an
optional version override. The workflow is self-contained (macOS runs on the standard `macos-15`
runner) and runs the scripts in this folder. The one outside dependency is the public
`storytold/craft-common-actions/actions/install-linux-deps` action (Linux build libraries, nfpm).
Push the tag only; do not create the release by hand, because the workflow creates the draft
and refuses to update a release that is already published.

| Platform | Script | Artifacts in `dist/release/` |
| --- | --- | --- |
| macOS universal | `macos/package.sh --arch universal` | `.dmg`, `.zip` (app), CLI `.tar.gz`; Developer ID codesign and notarization |
| Windows x64, x86, arm64 | `windows/package.ps1 -Arch <arch>` | WiX v5 `.msi`, portable `.zip`, CLI `.zip`; Azure Trusted Signing (or PFX) |
| Linux x86_64, aarch64 | `linux/package.sh` | `.AppImage`, `.deb`, `.rpm`, `.tar.gz`, CLI `.tar.gz` (nfpm) |
| Flatpak | `linux/flatpak-bundle.sh` | `.flatpak`  |

Not built: WebAssembly (AGENTS.md: there is no web build), riscv64, FreeBSD and Arch.

`env.sh` is shared setup (version from the root `Cargo.toml`, `dist/release`, application id
`io.github.zuramai.postguy`). Version overrides use `cargo xtask version set X.Y.Z`.

Signing secrets (set in the repository's `release` environment): `APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`;
`AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`,
`AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE` (or `WINDOWS_CERTIFICATE` and
`WINDOWS_CERTIFICATE_PASSWORD`). Without them the scripts warn and ship unsigned (ad-hoc signed on macOS), so releases work with no secrets.
The workflow creates a draft GitHub Release `v<version>`; publish it by hand. The `release`
environment must exist (Settings, Environments), even with no secrets in it.

Icons are derived at packaging time from `assets/logo`; no generated icon is committed.
