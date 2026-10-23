#!/usr/bin/env bash
# Check the artifacts package.sh left in $DIST: architectures, signatures, Gatekeeper and
# notarization tickets.
#
# Usage: packaging/macos/verify.sh [--arch universal|aarch64|x86_64] [--require-notarized]
#
# Ad-hoc signed artifacts (no certificate; non-release builds) get structure checks only, with a
# warning. On refs/heads/release, or with --require-notarized, a missing Developer ID signature
# or ticket is an error.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

ARCH=universal
REQUIRE=0
if [ "${GITHUB_REF:-}" = "refs/heads/release" ]; then REQUIRE=1; fi
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    --require-notarized) REQUIRE=1; shift ;;
    -h | --help) sed -n '2,9p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$ARCH" in
  universal) EXPECT="arm64 x86_64" ;;
  aarch64) EXPECT="arm64" ;;
  x86_64) EXPECT="x86_64" ;;
  *) echo "unknown --arch $ARCH" >&2; exit 2 ;;
esac

DMG="$DIST/postguy-$VERSION-macos-$ARCH.dmg"
APP_ZIP="$DIST/postguy-$VERSION-macos-$ARCH.zip"
CLI_TGZ="$DIST/postguy-cli-$VERSION-macos-$ARCH.tar.gz"
for f in "$DMG" "$APP_ZIP" "$CLI_TGZ"; do
  if [ ! -f "$f" ]; then echo "error: missing $f" >&2; exit 1; fi
done

TMP="$(mktemp -d)"
MOUNT="$TMP/mnt"
mkdir -p "$MOUNT"
cleanup() {
  hdiutil detach "$MOUNT" >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

# Fail unless `lipo -archs` of the binary lists exactly the expected architectures.
check_archs() {
  local got
  got="$(lipo -archs "$1" | tr ' ' '\n' | sort | tr '\n' ' ' | sed 's/ $//')"
  want="$(printf '%s\n' $EXPECT | sort | tr '\n' ' ' | sed 's/ $//')"
  if [ "$got" != "$want" ]; then
    echo "error: $1 has architectures '$got', expected '$want'" >&2
    exit 1
  fi
  echo "ok archs ($got): $1"
}

# Is this bundle/file ad-hoc signed (no Developer ID)?
is_adhoc() {
  local info
  info="$(codesign -dv "$1" 2>&1 || true)"
  case "$info" in *Signature=adhoc*) return 0 ;; *) return 1 ;; esac
}

# Verify one app bundle. $2 = label.
check_app() {
  local app="$1" label="$2"
  check_archs "$app/Contents/MacOS/Postguy"
  plutil -lint "$app/Contents/Info.plist" >/dev/null
  codesign --verify --deep --strict --verbose=2 "$app"
  if is_adhoc "$app"; then
    if [ "$REQUIRE" = 1 ]; then echo "error: $label is only ad-hoc signed" >&2; exit 1; fi
    warn "$label is ad-hoc signed: skipping Gatekeeper and notarization checks"
    return 0
  fi
  local info
  info="$(codesign -dv --verbose=4 "$app" 2>&1 || true)"
  case "$info" in *runtime*) ;; *) echo "error: $label lacks the hardened runtime" >&2; exit 1 ;; esac
  if xcrun stapler validate "$app"; then
    spctl --assess --type execute -vvv "$app"
  elif [ "$REQUIRE" = 1 ]; then
    echo "error: $label has no stapled notarization ticket" >&2
    exit 1
  else
    warn "$label has no stapled notarization ticket"
  fi
}

echo "==> app in $(basename "$APP_ZIP")"
ditto -x -k "$APP_ZIP" "$TMP/zip"
check_app "$TMP/zip/Postguy.app" "zipped Postguy.app"

echo "==> app in $(basename "$DMG")"
hdiutil attach -nobrowse -readonly -mountpoint "$MOUNT" "$DMG" >/dev/null
if [ "$(readlink "$MOUNT/Applications")" != "/Applications" ]; then
  echo "error: the DMG has no Applications link" >&2
  exit 1
fi
check_app "$MOUNT/Postguy.app" "Postguy.app in the DMG"
hdiutil detach "$MOUNT" >/dev/null
if is_adhoc "$DMG" || ! codesign -dv "$DMG" >/dev/null 2>&1; then
  if [ "$REQUIRE" = 1 ]; then echo "error: the DMG is not Developer ID signed" >&2; exit 1; fi
  warn "the DMG is not Developer ID signed: skipping its Gatekeeper check"
else
  codesign --verify --strict --verbose=2 "$DMG"
  if xcrun stapler validate "$DMG"; then
    spctl --assess --type open --context context:primary-signature -vvv "$DMG"
  elif [ "$REQUIRE" = 1 ]; then
    echo "error: the DMG has no stapled notarization ticket" >&2
    exit 1
  else
    warn "the DMG has no stapled notarization ticket"
  fi
fi

echo "==> CLI in $(basename "$CLI_TGZ")"
tar -xzf "$CLI_TGZ" -C "$TMP"
CLI="$TMP/postguy-cli-$VERSION-macos-$ARCH/postguy-cli"
check_archs "$CLI"
codesign --verify --strict --verbose=2 "$CLI"
if is_adhoc "$CLI"; then
  if [ "$REQUIRE" = 1 ]; then echo "error: postguy-cli is only ad-hoc signed" >&2; exit 1; fi
  warn "postguy-cli is ad-hoc signed"
fi

echo "==> verified"
