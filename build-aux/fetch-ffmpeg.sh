#!/usr/bin/env bash
# Fetch the pinned static FFmpeg sidecar (Linux) into
# src-tauri/binaries/<triple>/ffmpeg-<triple>.
# BtbN `linux64-gpl` (same vendor/pin style as build-aux/fetch-ffmpeg.ps1 on
# Windows): needs libx264 + NVENC for the save-time lanczos path, and aac for
# the capture mux fallback. Binary is glibc-dynamic only (no distro libs).
set -euo pipefail

# Bump the pin by updating these three values together (autobuild tag + asset).
BTBN_TAG="autobuild-2026-09-15-13-18"
BTBN_ASSET="ffmpeg-N-126574-g912208af28-linux64-gpl.tar.xz"
BTBN_SHA256="7c5b4fd54be55970595f3c62b22c8a502587abe61c3e5e37a2cb81630093bad4"
BTBN_SIZE=151537292

URL="https://github.com/BtbN/FFmpeg-Builds/releases/download/${BTBN_TAG}/${BTBN_ASSET}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
OUT_DIR="$ROOT/src-tauri/binaries/$TRIPLE"
OUT="$OUT_DIR/ffmpeg-$TRIPLE"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [ -x "$OUT" ]; then
  echo "==> already staged: $OUT"
  "$OUT" -hide_banner -version | head -1
  exit 0
fi

echo "==> downloading $BTBN_ASSET"
curl -sL --fail --max-time 900 -o "$WORK/$BTBN_ASSET" "$URL"

size=$(stat -c %s "$WORK/$BTBN_ASSET")
if [ "$size" != "$BTBN_SIZE" ]; then
  echo "error: size mismatch (got $size, want $BTBN_SIZE)" >&2
  exit 1
fi
echo "$BTBN_SHA256  $WORK/$BTBN_ASSET" | sha256sum -c -

echo "==> extracting"
tar -xf "$WORK/$BTBN_ASSET" -C "$WORK" --wildcards '*/bin/ffmpeg'
ff=$(find "$WORK" -path '*/bin/ffmpeg' -type f | head -1)
if [ -z "$ff" ]; then
  echo "error: ffmpeg not found inside archive" >&2
  exit 1
fi

echo "==> verifying required encoders"
missing=0
for enc in libx264 h264_nvenc hevc_nvenc aac; do
  if ! "$ff" -hide_banner -encoders 2>/dev/null | grep -q " $enc "; then
    echo "error: encoder '$enc' missing from this build" >&2
    missing=1
  fi
done
if [ "$missing" != 0 ]; then
  exit 1
fi

echo "==> staging to $OUT"
mkdir -p "$OUT_DIR"
install -m 755 "$ff" "$OUT"
"$OUT" -hide_banner -version | head -1
echo "OK: $OUT"
