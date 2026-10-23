#!/usr/bin/env bash
# Build, sign and (optionally) notarize the macOS release artifacts:
#
#   $DIST/postguy-<version>-macos-<arch>.dmg             Postguy.app on a drag-to-Applications DMG
#   $DIST/postguy-<version>-macos-<arch>.zip             Postguy.app (ditto zip, stapled when notarized)
#   $DIST/postguy-cli-<version>-macos-<arch>.tar.gz      the headless CLI
#
# Usage: packaging/macos/package.sh [--arch universal|aarch64|x86_64] [--skip-build]
#
# Signing (env):
#   MACOS_SIGN_IDENTITY   codesign identity (name or SHA-1). Default "-" = ad-hoc (local testing;
#                         Gatekeeper rejects the result on other Macs). In CI, import-cert.sh sets it.
#   MACOS_KEYCHAIN        keychain holding the identity (optional)
# Notarization (env; all three needed, and a real identity):
#   APPLE_ID, APPLE_PASSWORD (app-specific password), APPLE_TEAM_ID
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/macos"

ARCH=universal
SKIP_BUILD=0
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h | --help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$ARCH" in
  universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  aarch64) TARGETS=(aarch64-apple-darwin) ;;
  x86_64) TARGETS=(x86_64-apple-darwin) ;;
  *) echo "unknown --arch $ARCH" >&2; exit 2 ;;
esac

# Keep in sync with LSMinimumSystemVersion in Info.plist.in.
export MACOSX_DEPLOYMENT_TARGET=11.0
IDENTITY="${MACOS_SIGN_IDENTITY:--}"
SHORT_VERSION="${VERSION%%-*}"
BUILD_SHA="${BUILD_SHA:-${POSTGUY_BUILD_SHA:-unknown}}"
WORK="$CARGO_TARGET_DIR/macos-package"
APP="$WORK/Postguy.app"
DMG="$DIST/postguy-$VERSION-macos-$ARCH.dmg"
APP_ZIP="$DIST/postguy-$VERSION-macos-$ARCH.zip"
CLI_TGZ="$DIST/postguy-cli-$VERSION-macos-$ARCH.tar.gz"

NOTARIZE=0
if [ "$IDENTITY" = "-" ]; then
  warn "macOS: ad-hoc signing (no MACOS_SIGN_IDENTITY); artifacts are not notarized"
elif [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  NOTARIZE=1
else
  warn "macOS: APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID incomplete; signed but not notarized"
fi

echo "==> Postguy $VERSION for macOS ($ARCH), identity: $IDENTITY, notarize: $NOTARIZE"

# ---- build -------------------------------------------------------------------------------------
if [ "$SKIP_BUILD" = 0 ]; then
  args=()
  for t in "${TARGETS[@]}"; do args+=(--target "$t"); done
  (cd "$ROOT" && cargo build --release --locked -p postguy -p postguy-cli "${args[@]}")
fi

rm -rf "$WORK"
mkdir -p "$WORK/bin"
for bin in postguy postguy-cli; do
  inputs=()
  for t in "${TARGETS[@]}"; do inputs+=("$CARGO_TARGET_DIR/$t/release/$bin"); done
  lipo -create -output "$WORK/bin/$bin" "${inputs[@]}"
  lipo -info "$WORK/bin/$bin"
done

# ---- helpers -----------------------------------------------------------------------------------
sign() {
  # Hardened runtime + secure timestamp for a real identity; ad-hoc can't be timestamped.
  local ts=(--timestamp) kc=()
  if [ "$IDENTITY" = "-" ]; then ts=(--timestamp=none); fi
  if [ -n "${MACOS_KEYCHAIN:-}" ]; then kc=(--keychain "$MACOS_KEYCHAIN"); fi
  codesign --force --sign "$IDENTITY" ${kc[@]+"${kc[@]}"} "${ts[@]}" "$@"
}

notarize() {
  local file="$1" out id status
  echo "==> notarizing $(basename "$file") (this can take a few minutes)"
  out="$(xcrun notarytool submit "$file" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" --wait --timeout 1h --output-format json)" || true
  echo "$out"
  id="$(printf '%s' "$out" | plutil -extract id raw -o - - 2>/dev/null || true)"
  status="$(printf '%s' "$out" | plutil -extract status raw -o - - 2>/dev/null || true)"
  if [ "$status" != "Accepted" ]; then
    if [ -n "$id" ]; then
      xcrun notarytool log "$id" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" || true
    fi
    echo "error: notarization of $(basename "$file") failed (status: ${status:-unknown})" >&2
    exit 1
  fi
}

# hdiutil create/attach occasionally fail on CI runners ("Resource busy"): try five times.
# Usage: hdiutil_retry <what> <hdiutil args...>
hdiutil_retry() {
  local what="$1" attempt
  shift
  for attempt in 1 2 3 4 5; do
    if hdiutil "$@"; then
      return 0
    fi
    if [ "$attempt" = 5 ]; then
      echo "hdiutil $what failed 5 times" >&2
      return 1
    fi
    warn "hdiutil $what failed (attempt $attempt of 5); retrying in $((attempt * 10))s"
    sleep $((attempt * 10))
  done
}

# ---- icon: icns from the contributor-original PNGs in assets/logo ------------------------------
# iconutil needs the iconset names; the sizes we have (16..512) cover everything up to 256@2x and
# 512@1x. There is no 1024 px master, so 512@2x is left out rather than upscaled.
echo "==> building Postguy.icns"
ICONSET="$WORK/Postguy.iconset"
mkdir -p "$ICONSET"
LOGO="$ROOT/assets/logo"
cp "$LOGO/postguy-16.png" "$ICONSET/icon_16x16.png"
cp "$LOGO/postguy-32.png" "$ICONSET/icon_16x16@2x.png"
cp "$LOGO/postguy-32.png" "$ICONSET/icon_32x32.png"
cp "$LOGO/postguy-64.png" "$ICONSET/icon_32x32@2x.png"
cp "$LOGO/postguy-128.png" "$ICONSET/icon_128x128.png"
cp "$LOGO/postguy-256.png" "$ICONSET/icon_128x128@2x.png"
cp "$LOGO/postguy-256.png" "$ICONSET/icon_256x256.png"
cp "$LOGO/postguy-512.png" "$ICONSET/icon_256x256@2x.png"
cp "$LOGO/postguy-512.png" "$ICONSET/icon_512x512.png"
iconutil --convert icns --output "$WORK/Postguy.icns" "$ICONSET"

# ---- Postguy.app -------------------------------------------------------------------------------
echo "==> assembling $APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# Executable and icon carry the display name (CFBundleExecutable / CFBundleIconFile).
cp "$WORK/bin/postguy" "$APP/Contents/MacOS/Postguy"
cp "$WORK/Postguy.icns" "$APP/Contents/Resources/Postguy.icns"
copy_docs "$APP/Contents/Resources"
sed -e "s/@APP_ID@/$APP_ID/g" -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" \
  -e "s/@BUILD_SHA@/$BUILD_SHA/g" \
  "$HERE/Info.plist.in" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"

# Sign inside-out: nested code first, then the bundle (no --deep on the final signature).
# Today the only nested code is the main executable.
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP/Contents/MacOS/Postguy"
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

if [ "$NOTARIZE" = 1 ]; then
  ditto -c -k --keepParent "$APP" "$WORK/Postguy-notarize.zip"
  notarize "$WORK/Postguy-notarize.zip"
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
  spctl --assess --type execute -vvv "$APP"
fi

# ---- app zip (after stapling, so the ticket is inside) -----------------------------------------
echo "==> building $APP_ZIP"
rm -f "$APP_ZIP"
ditto -c -k --keepParent "$APP" "$APP_ZIP"

# ---- DMG ---------------------------------------------------------------------------------------
echo "==> building $DMG"
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/Postguy.app"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
# `create -srcfolder` writes the files as they are (no Finder metadata that would break
# `codesign --verify --strict` on the app inside). No background image: Finder's default layout.
hdiutil_retry create create -srcfolder "$STAGE" -volname "Postguy" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$DMG"
sign "$DMG"
codesign --verify --strict --verbose=2 "$DMG"
# The app inside the image must still pass --strict.
MOUNT="$WORK/dmg-check"
rm -rf "$MOUNT"
mkdir -p "$MOUNT"
hdiutil_retry attach attach -nobrowse -readonly -mountpoint "$MOUNT" "$DMG" >/dev/null
if ! codesign --verify --strict --deep --verbose=2 "$MOUNT/Postguy.app"; then
  hdiutil_retry detach detach "$MOUNT" >/dev/null || true
  exit 1
fi
hdiutil_retry detach detach "$MOUNT" >/dev/null
if [ "$NOTARIZE" = 1 ]; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
  xcrun stapler validate "$DMG"
  spctl --assess --type open --context context:primary-signature -vvv "$DMG"
fi

# ---- CLI ---------------------------------------------------------------------------------------
echo "==> building $CLI_TGZ"
CLI_NAME="postguy-cli-$VERSION-macos-$ARCH"
CLI_DIR="$WORK/$CLI_NAME"
mkdir -p "$CLI_DIR"
cp "$WORK/bin/postguy-cli" "$CLI_DIR/"
copy_docs "$CLI_DIR"
sign --options runtime "$CLI_DIR/postguy-cli"
codesign --verify --strict --verbose=2 "$CLI_DIR/postguy-cli"
# A bare Mach-O can't carry a stapled ticket, so notarize a zip of the folder (Gatekeeper looks
# the ticket up online); the shipped archive is the tar.gz of the same signed binary.
if [ "$NOTARIZE" = 1 ]; then
  ditto -c -k --keepParent "$CLI_DIR" "$WORK/$CLI_NAME-notarize.zip"
  notarize "$WORK/$CLI_NAME-notarize.zip"
fi
rm -f "$CLI_TGZ"
COPYFILE_DISABLE=1 tar -C "$WORK" -czf "$CLI_TGZ" "$CLI_NAME"

# Smoke test: the CLI has no --version; `tools` lists the automation tool table.
"$CLI_DIR/postguy-cli" tools >/dev/null
echo "==> done"
ls -lh "$DMG" "$APP_ZIP" "$CLI_TGZ"
