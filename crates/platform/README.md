# postguy-platform

- **Layer:** L7 (checked by `cargo xtask layers`)

OS services: config/data dirs, file dialogs, reveal in file manager, open URL, clipboard, system proxy, system font fallback.

- `update` — installs app updates the engine downloaded and verified: `Install::detect` tells a
  macOS bundle, Windows MSI or portable zip, AppImage or Linux tarball from package-manager
  installs (Flatpak, deb, rpm) and development builds; `Install::prepare` unpacks the package and,
  when the running app is signed, requires the same Developer ID team or Authenticode subject;
  `PreparedUpdate::apply_and_relaunch` swaps it in after the app exited and starts it again.

Conventions (AGENTS.md): `Result` everywhere, no panic paths, no `unsafe`, checked
arithmetic and capped allocations on anything input-sized; errors are actionable.
See the workspace README.
