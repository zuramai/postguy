# Postguy

Postguy is a clean-room, offline, native API client for HTTP, gRPC and WebSocket, written in Rust
with egui. It does what the essential parts of Postman do — collections, environments, requests,
import/export — and nothing online: no account, no sync, no telemetry. It runs natively on macOS,
Windows, Linux and FreeBSD.

See [AGENTS.md](AGENTS.md) for the rules that bind every contributor and
[docs/architecture.md](docs/architecture.md) for the crate layout.

## Status

Desktop HTTP client with collections, environments, variables, request builder, response viewer,
imports (Postman, OpenAPI 3 / Swagger 2 in JSON or YAML, HAR, `.proto` files, cURL, zip documents),
code snippets, export dialogs and the common auth types. Pre-request and post-response scripts
(`pm.test`, `pm.expect`, a sandboxed QuickJS engine) and a collection runner with data files, a
Runs tab and a `postguy-cli run --collection` command with JUnit/HTML/JSON reports are included.
WebSocket and gRPC clients exist at a basic level (scripts and the runner cover HTTP requests
only). Feature-by-feature state is in `parity/postman-features.toml`; known gaps are in
[docs/gaps.md](docs/gaps.md).

## Install

Download from the [latest release](https://github.com/zuramai/postguy/releases/latest). Builds are
not code-signed or notarized, so macOS and Windows show a warning for files opened by hand; the steps
below avoid or get past it. Each file has a checksum in `SHA256SUMS.txt`.

Postguy keeps its data in a workspace folder: `~/postguy-workspace` (`%USERPROFILE%\postguy-workspace` on
Windows), or `./postguy-workspace` if that exists in the folder you start it from. Pass `--workspace DIR` to
choose another.

### macOS (Apple Silicon and Intel)

Download with `curl`, which does not mark the file as quarantined, so Gatekeeper does not block it:

```sh
VERSION=$(curl -fsSL https://api.github.com/repos/zuramai/postguy/releases/latest | grep -m1 '"tag_name"' | sed -E 's/.*"v([^"]+)".*/\1/')
curl -fL -o /tmp/postguy.zip "https://github.com/zuramai/postguy/releases/download/v$VERSION/postguy-$VERSION-macos-universal.zip"
ditto -x -k /tmp/postguy.zip /Applications
open /Applications/Postguy.app
```

If you downloaded the `.dmg` or `.zip` in a browser and macOS says it "could not verify" the app, run
`xattr -dr com.apple.quarantine /Applications/Postguy.app` (or the downloaded file), or open System
Settings, Privacy & Security and choose **Open Anyway**.

The headless CLI: `curl -fL "https://github.com/zuramai/postguy/releases/download/v$VERSION/postguy-cli-$VERSION-macos-universal.tar.gz" | tar -xz`.

### Windows

Download `postguy-<version>-windows-x64.msi` (`x86` or `arm64` for those machines) and run it. If
SmartScreen says "Windows protected your PC", choose **More info**, then **Run anyway**. The installer
adds Postguy to the Start menu and `postguy-cli` to the `PATH` (clear that in the feature tree to
skip it). Silent install: `msiexec /i postguy-<version>-windows-x64.msi /qn`. A portable `.zip` is
also published.

### Linux (x86_64 and aarch64)

Pick one:

```sh
# AppImage (any distribution)
chmod +x postguy-<version>-linux-x86_64.AppImage && ./postguy-<version>-linux-x86_64.AppImage

# Debian, Ubuntu
sudo apt install ./postguy-<version>-linux-x86_64.deb

# Fedora, openSUSE
sudo dnf install ./postguy-<version>-linux-x86_64.rpm

# Flatpak (keeps its workspace in ~/.var/app/io.github.zuramai.postguy)
flatpak install --user ./postguy-<version>-linux-x86_64.flatpak

# Tarball
tar -xzf postguy-<version>-linux-x86_64.tar.gz
```

Use `aarch64` in the file name on ARM machines. `postguy-cli-<version>-linux-<arch>.tar.gz` is the
headless CLI alone.

### From source

See Building below.

## Building

```
cargo build -p postguy            # desktop app
cargo build -p postguy-cli        # headless CLI (run --script, tools/call, ui); import/export via call
cargo xtask ci                    # fmt, clippy -D warnings, tests, layers, assets, parity
```

Every user-facing feature is reachable headlessly through `postguy-cli` (`cargo run -p
postguy-cli -- run --script steps.json --root DIR`); the desktop UI is layered on the same engine.

## Licence

MIT OR Apache-2.0. Bundled third-party material is listed in [ATTRIBUTION.md](ATTRIBUTION.md)
and [NOTICE](NOTICE).
