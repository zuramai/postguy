# shellcheck shell=bash
# Shared setup for the packaging scripts. Source it: `. "$(dirname "$0")/../env.sh"`.
#
# Exports:
#   ROOT                workspace root
#   VERSION             [workspace.package] version from Cargo.toml (override: POSTGUY_VERSION)
#   DIST                output directory for release artifacts (default: $ROOT/dist/release)
#   POSTGUY_BUILD_SHA   git commit of the build
#   POSTGUY_BUILD_DATE  UTC build date, YYYY-MM-DD
#   CARGO_TARGET_DIR    cargo's target dir (default: $ROOT/target)
#   APP_ID              reverse-DNS application id used for bundle ids, desktop files and installers

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export ROOT

APP_ID="io.github.zuramai.postguy"
export APP_ID

# The version lives in exactly one place: `[workspace.package] version` in the root Cargo.toml
# (`cargo xtask version` prints the same thing; awk avoids compiling xtask here).
workspace_version() {
  awk '
    /^\[/ { in_pkg = ($0 == "[workspace.package]") ; next }
    in_pkg && $1 == "version" { gsub(/[" ]/, "", $3); print $3; exit }
  ' "$ROOT/Cargo.toml"
}

VERSION="${POSTGUY_VERSION:-$(workspace_version)}"
if [ -z "$VERSION" ]; then
  echo "error: could not read [workspace.package] version from $ROOT/Cargo.toml" >&2
  exit 1
fi
export VERSION

DIST="${DIST:-$ROOT/dist/release}"
mkdir -p "$DIST"
export DIST

if [ -z "${POSTGUY_BUILD_SHA:-}" ]; then
  POSTGUY_BUILD_SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || true)"
fi
export POSTGUY_BUILD_SHA
export POSTGUY_BUILD_DATE="${POSTGUY_BUILD_DATE:-$(date -u +%Y-%m-%d)}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

# Emit a GitHub Actions warning (plain stderr outside Actions).
warn() {
  if [ -n "${GITHUB_ACTIONS:-}" ]; then echo "::warning::$*"; else echo "warning: $*" >&2; fi
}

# Copy licence and readme files that exist into a package directory, with the licence of the
# bundled Poppins font (OFL-1.1, assets/fonts) that the app embeds.
copy_docs() {
  local dest="$1" f
  for f in README.md LICENSE-MIT LICENSE-APACHE NOTICE ATTRIBUTION.md; do
    if [ -f "$ROOT/$f" ]; then cp "$ROOT/$f" "$dest/"; fi
  done
  if [ -f "$ROOT/assets/fonts/LICENSE-poppins.txt" ]; then cp "$ROOT/assets/fonts/LICENSE-poppins.txt" "$dest/OFL-Poppins.txt"; fi
}

# Portable SHA-256 of a file (prints just the hash).
sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}
