#!/usr/bin/env bash
# Fetch the pinned embedded OBS Studio + obs-cmd for Linux (MoonClip V3
# capture engine). End users never install OBS separately.
#
# NOTE (best effort): OBS does not publish a portable Linux tarball, so this
# script unpacks the official Ubuntu .deb into a private prefix and relies on
# the distro's shared libraries. If the unpacked build does not run on your
# distro, MoonClip falls back to the system `obs` binary (still isolated via
# --config-dir; see os/linux/binary.rs).
#
# Usage:  bash build-aux/fetch-obs.sh [--force]
# Env overrides: OBS_VERSION, OBS_DEB_ASSET, OBS_DEB_SHA256,
#                OBSCMD_VERSION, OBSCMD_ASSET, OBSCMD_SHA256
set -euo pipefail

FORCE=0
[ "${1:-}" = "--force" ] && FORCE=1

OBS_VERSION="${OBS_VERSION:-32.2.2}"
OBS_DEB_ASSET="${OBS_DEB_ASSET:-OBS-Studio-32.2.2-Ubuntu-24.04-x86_64.deb}"
OBS_DEB_SHA256="${OBS_DEB_SHA256:-b6557ca2059287210332accc94c267094050489cffffc5c832e746e3a418dab0}"

OBSCMD_VERSION="${OBSCMD_VERSION:-v1.0.2}"
OBSCMD_ASSET="${OBSCMD_ASSET:-obs-cmd-x64-linux.tar.gz}"
OBSCMD_SHA256="${OBSCMD_SHA256:-b7e022a80bf75d8c82b9549e977f36c439ccd2711ffde776cae16874d28cd4cf}"

TRIPLE="x86_64-unknown-linux-gnu"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$ROOT/src-tauri/binaries/$TRIPLE"
OBS_ROOT="$OUT_DIR/obs"
OBS_BIN="$OBS_ROOT/bin/obs"
CMD_BIN="$OUT_DIR/obs-cmd"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [ -x "$OBS_BIN" ] && [ -x "$CMD_BIN" ] && [ "$FORCE" -eq 0 ]; then
  echo "OK (cached): $OBS_BIN"
  "$OBS_BIN" --version | head -1 || true
  echo "OK (cached): $CMD_BIN"
  exit 0
fi

mkdir -p "$OUT_DIR"
need() { command -v "$1" >/dev/null 2>&1 || { echo "missing tool: $1" >&2; exit 1; }; }
need curl
need sha256sum
need tar

# --- obs-cmd -----------------------------------------------------------------
if [ ! -x "$CMD_BIN" ] || [ "$FORCE" -eq 1 ]; then
  URL="https://github.com/grigio/obs-cmd/releases/download/$OBSCMD_VERSION/$OBSCMD_ASSET"
  echo "==> downloading $URL"
  curl -fL "$URL" -o "$WORK/$OBSCMD_ASSET"
  echo "$OBSCMD_SHA256  $WORK/$OBSCMD_ASSET" | sha256sum -c -
  tar -xzf "$WORK/$OBSCMD_ASSET" -C "$WORK"
  BIN="$(find "$WORK" -maxdepth 3 -type f -name 'obs-cmd' | head -1)"
  [ -n "$BIN" ] || { echo "obs-cmd binary not found in $OBSCMD_ASSET" >&2; exit 1; }
  install -m 0755 "$BIN" "$CMD_BIN"
  "$CMD_BIN" --version | head -1 || true
fi

# --- OBS Studio (best-effort .deb unpack) ------------------------------------
if [ ! -x "$OBS_BIN" ] || [ "$FORCE" -eq 1 ]; then
  need ar
  URL="https://github.com/obsproject/obs-studio/releases/download/$OBS_VERSION/$OBS_DEB_ASSET"
  echo "==> downloading $URL"
  curl -fL "$URL" -o "$WORK/obs.deb"
  echo "$OBS_DEB_SHA256  $WORK/obs.deb" | sha256sum -c -
  mkdir -p "$WORK/deb"
  (cd "$WORK/deb" && ar x "$WORK/obs.deb")
  DATA="$(find "$WORK/deb" -maxdepth 1 -name 'data.tar.*' | head -1)"
  [ -n "$DATA" ] || { echo "data.tar not found in deb" >&2; exit 1; }
  tar -xf "$DATA" -C "$WORK/deb"
  rm -rf "$OBS_ROOT"
  mkdir -p "$OBS_ROOT/bin" "$OBS_ROOT/lib" "$OBS_ROOT/share"
  # bin/obs + share/obs
  [ -e "$WORK/deb/usr/bin/obs" ] && cp -a "$WORK/deb/usr/bin/obs" "$OBS_ROOT/bin/obs"
  [ -d "$WORK/deb/usr/share/obs" ] && cp -a "$WORK/deb/usr/share/obs" "$OBS_ROOT/share/obs"
  # plugins: flatten lib/<multiarch>/obs-plugins -> lib/obs-plugins
  PLUG="$(find "$WORK/deb/usr/lib" -maxdepth 2 -type d -name 'obs-plugins' 2>/dev/null | head -1)"
  [ -n "$PLUG" ] && cp -a "$PLUG" "$OBS_ROOT/lib/obs-plugins"
  [ -d "$OBS_ROOT/bin" ] || { echo "OBS layout unexpected after unpack" >&2; exit 1; }
  : > "$OBS_ROOT/portable_mode.txt"
  echo "WARN: unpacked Linux OBS depends on distro libraries (Qt, pipewire, ffmpeg)."
  echo "      If it fails to start, install OBS from your distro; MoonClip will use it"
  echo "      with --config-dir (still fully isolated)."
  "$OBS_BIN" --version | head -1 || true
fi

echo "OK: $OBS_ROOT"
echo "OK: $CMD_BIN"
