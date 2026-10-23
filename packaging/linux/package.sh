#!/usr/bin/env bash
# Build and package Postguy for Linux (<arch> is x86_64 or aarch64):
#
#   $DIST/postguy-<version>-linux-<arch>.AppImage       any distro with glibc >= the build host's
#   $DIST/postguy-<version>-linux-<arch>.AppImage.zsync delta updates (needs zsyncmake)
#   $DIST/postguy-<version>-linux-<arch>.deb            Debian, Ubuntu, Mint, Pop!_OS, ...
#   $DIST/postguy-<version>-linux-<arch>.rpm            Fedora, openSUSE, RHEL, ...
#   $DIST/postguy-<version>-linux-<arch>.tar.gz         plain FHS-style tree (bin/, share/)
#   $DIST/postguy-cli-<version>-linux-<arch>.tar.gz     the headless CLI alone (servers, CI, agents)
#
# Usage: packaging/linux/package.sh [--skip-build] [--formats "appimage deb rpm tar cli"]
#
# Needs: cargo, curl; nfpm for deb/rpm (https://nfpm.goreleaser.com); appimagetool for the
# AppImage (a pinned release, downloaded into $CARGO_TARGET_DIR and checked against its SHA-256 if
# missing; override with APPIMAGETOOL=/path). Build on an old distro (CI: Ubuntu 22.04, glibc
# 2.35) so the binaries run on newer ones. Optional: desktop-file-validate, appstreamcli,
# zsyncmake (the zsync package) for the AppImage's .zsync.
#
# Environment: VERSION / BUILD_DATE (set by the release workflow), POSTGUY_MAINTAINER,
# GITHUB_REPOSITORY (the AppImage update information points at that repository's releases).
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/linux"
BUILD_DATE_STAMP="${BUILD_DATE:-$POSTGUY_BUILD_DATE}"

SKIP_BUILD=0
FORMATS="appimage deb rpm tar cli"
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --formats) FORMATS="${2:?--formats needs a value}"; shift 2 ;;
    -h | --help) sed -n '2,19p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) DEB_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64; DEB_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
export POSTGUY_MAINTAINER="${POSTGUY_MAINTAINER:-Postguy maintainers <noreply@users.noreply.github.com>}"
BASENAME="postguy-$VERSION-linux-$ARCH"

echo "==> Postguy $VERSION for Linux $ARCH ($FORMATS)"

if [ "$SKIP_BUILD" = 0 ]; then
  (cd "$ROOT" && cargo build --release --locked -p postguy -p postguy-cli)
fi
BIN="$CARGO_TARGET_DIR/release"
WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- stage an FHS tree (shared by every format) -------------------------------------------------
install -Dm755 "$BIN/postguy" "$STAGE/usr/bin/postguy"
install -Dm755 "$BIN/postguy-cli" "$STAGE/usr/bin/postguy-cli"
strip "$STAGE/usr/bin/postguy" "$STAGE/usr/bin/postguy-cli" 2>/dev/null || true
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
mkdir -p "$STAGE/usr/share/metainfo"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@DATE@/$BUILD_DATE_STAMP/g" \
  "$HERE/$APP_ID.metainfo.xml.in" >"$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
# Icons: the committed logo PNGs and SVG, placed into the hicolor theme (nothing new is committed).
for size in 16 32 64 128 256 512; do
  install -Dm644 "$ROOT/assets/logo/postguy-$size.png" "$STAGE/usr/share/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
done
install -Dm644 "$ROOT/assets/logo/postguy.svg" "$STAGE/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
mkdir -p "$STAGE/usr/share/doc/postguy"
copy_docs "$STAGE/usr/share/doc/postguy"

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi
if command -v appstreamcli >/dev/null; then
  appstreamcli validate --no-net --explain "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
fi

has() { case " $FORMATS " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# ---- .tar.gz ------------------------------------------------------------------------------------
if has tar; then
  mkdir -p "$WORK/tar"
  cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
  tar -C "$WORK/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
  echo "wrote $DIST/$BASENAME.tar.gz"
fi

# ---- CLI-only .tar.gz ---------------------------------------------------------------------------
# The stripped postguy-cli with the licences, for machines that never open a window. Like the
# other formats it needs glibc >= the build host's.
if has cli; then
  CLI_NAME="postguy-cli-$VERSION-linux-$ARCH"
  CLI_DIR="$WORK/cli/$CLI_NAME"
  mkdir -p "$CLI_DIR"
  cp "$STAGE/usr/bin/postguy-cli" "$CLI_DIR/"
  copy_docs "$CLI_DIR"
  tar -C "$WORK/cli" -czf "$DIST/$CLI_NAME.tar.gz" "$CLI_NAME"
  echo "wrote $DIST/$CLI_NAME.tar.gz"
fi

# ---- .deb / .rpm --------------------------------------------------------------------------------
if has deb || has rpm; then
  command -v nfpm >/dev/null || { echo "error: nfpm not found (https://nfpm.goreleaser.com/install/)" >&2; exit 1; }
  export VERSION
  export RELEASE="1"
  export NFPM_ARCH="$DEB_ARCH"
  # nfpm expands env vars in fields like `version` and `arch`, but not in `contents[].src`.
  sed "s|\${STAGE}|$STAGE|g" "$HERE/nfpm.yaml" >"$WORK/nfpm.yaml"
  for fmt in deb rpm; do
    if has "$fmt"; then (cd "$ROOT" && nfpm package -f "$WORK/nfpm.yaml" -p "$fmt" -t "$DIST/$BASENAME.$fmt"); fi
  done
  for fmt in deb rpm; do
    if has "$fmt"; then echo "wrote $DIST/$BASENAME.$fmt"; fi
  done
fi

# ---- AppImage -----------------------------------------------------------------------------------
if has appimage; then
  APPDIR="$WORK/Postguy.AppDir"
  cp -R "$STAGE" "$APPDIR"
  rm -rf "$APPDIR/usr/share/doc"
  ln -s usr/bin/postguy "$APPDIR/AppRun"
  cp "$HERE/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
  cp "$ROOT/assets/logo/postguy-256.png" "$APPDIR/$APP_ID.png"
  ln -s "$APP_ID.png" "$APPDIR/.DirIcon"

  TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
  if [ -z "$TOOL" ]; then
    # A pinned release (not `continuous`) so the checksum below stays valid.
    TOOL_VERSION=1.9.1
    case "$ARCH" in
      x86_64) TOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0 ;;
      aarch64) TOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158 ;;
      *) echo "error: no appimagetool checksum for $ARCH" >&2; exit 1 ;;
    esac
    TOOL="$CARGO_TARGET_DIR/appimagetool-$TOOL_VERSION-$ARCH.AppImage"
    if [ ! -x "$TOOL" ] || [ "$(sha256 "$TOOL")" != "$TOOL_SHA256" ]; then
      mkdir -p "$CARGO_TARGET_DIR"
      curl --proto '=https' --proto-redir =https -fsSL -o "$TOOL" \
        "https://github.com/AppImage/appimagetool/releases/download/$TOOL_VERSION/appimagetool-$ARCH.AppImage"
      if [ "$(sha256 "$TOOL")" != "$TOOL_SHA256" ]; then
        rm -f "$TOOL"
        echo "error: appimagetool $TOOL_VERSION ($ARCH) failed its SHA-256 check" >&2
        exit 1
      fi
      chmod +x "$TOOL"
    fi
  fi
  # Absolute, because appimagetool runs in $DIST below (CARGO_TARGET_DIR or APPIMAGETOOL may be
  # relative).
  OUT="$(cd "$DIST" && pwd)/$BASENAME.AppImage"
  APPDIR="$(cd "$APPDIR" && pwd)"
  TOOL="$(cd "$(dirname "$TOOL")" && pwd)/$(basename "$TOOL")"
  # A .zsync left by an earlier run would hide a missing zsyncmake and describe another file.
  rm -f "$OUT.zsync"
  # Update information: AppImageUpdate and the like read it from the file and fetch only the
  # blocks that changed in a newer release, through the .zsync published next to each AppImage on
  # GitHub Releases. This is read by an updater the user runs; Postguy itself never checks.
  REPO="${GITHUB_REPOSITORY:-zuramai/postguy}"
  UPDATE_INFO="gh-releases-zsync|${REPO%%/*}|${REPO#*/}|latest|postguy-*-linux-$ARCH.AppImage.zsync"
  # Extract-and-run: works without FUSE (containers, CI). The output embeds the static runtime,
  # so users don't need libfuse2 either. With zsyncmake on the host appimagetool also writes the
  # .zsync, into its working directory, hence the cd.
  (cd "$DIST" && ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream -u "$UPDATE_INFO" "$APPDIR" "$OUT")
  echo "wrote $OUT"
  if [ -s "$OUT.zsync" ]; then
    echo "wrote $OUT.zsync"
  else
    warn "zsyncmake not found, so $OUT.zsync was not written; AppImage delta updates need it"
  fi
fi

# The headless CLI is the only binary that runs without a display.
"$STAGE/usr/bin/postguy-cli" tools >/dev/null
echo "==> done"
ls -lh "$DIST"
