#!/usr/bin/env bash
# Build the pinned OBS Studio from source for Linux and stage it as MoonClip's
# embedded capture engine (`src-tauri/binaries/<triple>/obs/`).
#
# Why from source: OBS ships no portable Linux tarball, and the Ubuntu .deb
# depends on Ubuntu sonames (libavcodec 60, Qt 6.4) that do not exist on
# Fedora. A distro-native build links the distro's libraries and works.
#
# Isolation (runtime, see os/linux/obs.rs): MoonClip launches this copy with
# `XDG_CONFIG_HOME=<MoonClip data dir>/obs/config`, so OBS writes its config
# there and NEVER touches `~/.config/obs-studio` (the user's own OBS).
#
# Usage:  bash build-aux/linux/build-obs.sh [--force]
# Env overrides: OBS_VERSION, BUILD_JOBS, OBS_BUILD_DIR
#
# Identity: a MoonClip patch (build-aux/patches/) renames the engine binaries
# and scrubs OBS fingerprints; PATCH_REV invalidates the staged cache when the
# patch set changes.
set -euo pipefail

FORCE=0
[ "${1:-}" = "--force" ] && FORCE=1

OBS_VERSION="${OBS_VERSION:-32.2.2}"
BUILD_JOBS="${BUILD_JOBS:-$(nproc)}"
PATCH_REV="2"

TRIPLE="x86_64-unknown-linux-gnu"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/src-tauri/binaries/$TRIPLE/engine"
OBS_BIN="$OUT/bin/moonclip-engine"
PATCH="$ROOT/build-aux/patches/0001-identity.patch"
# Persistent build cache: ninja resumes instead of recompiling after a failed
# post-build step. `--force` wipes it.
WORK="${OBS_BUILD_DIR:-$HOME/.cache/MoonClip/obs-build}"
SRC="$WORK/obs-studio"

need() { command -v "$1" >/dev/null 2>&1 || { echo "missing tool: $1" >&2; exit 1; }; }

if [ -f "$OUT/.moonclip-staged" ] && [ "$(cat "$OUT/.moonclip-staged")" = "patch=$PATCH_REV" ] && [ "$FORCE" -eq 0 ]; then
  echo "OK (cached): $OBS_BIN"
  XDG_CONFIG_HOME="$(mktemp -d)" "$OBS_BIN" --version | head -1 || true
  exit 0
fi

need git
need cmake
need ninja

mkdir -p "$WORK"
if [ "$FORCE" -eq 1 ]; then
  rm -rf "$SRC"
fi

# GitHub source tarballs omit submodules (obs-browser, obs-websocket) and
# CMake hard-fails without them, so clone the pinned tag recursively.
# Submodule commits are pinned by the tag's gitlinks.
if [ ! -d "$SRC/.git" ]; then
  echo "==> cloning obs-studio $OBS_VERSION (recursive, shallow)"
  git clone --quiet --depth 1 --branch "$OBS_VERSION" \
    --recurse-submodules --shallow-submodules \
    https://github.com/obsproject/obs-studio.git "$SRC"
else
  echo "==> reusing cached source: $SRC"
fi

echo "==> applying MoonClip identity patch (rev $PATCH_REV)"
git -C "$SRC" checkout -- .
git -C "$SRC" apply "$PATCH"

echo "==> configuring (minimal plugin set, distro libs, $BUILD_JOBS jobs)"
cmake -S "$SRC" -B "$SRC/build" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/nonexistent/moonclip-obs \
  -DOBS_VERSION_OVERRIDE="$OBS_VERSION" \
  -DENABLE_RELOCATABLE=ON \
  -DENABLE_BROWSER=OFF \
  -DENABLE_AJA=OFF \
  -DENABLE_DECKLINK=OFF \
  -DENABLE_SYPHON=OFF \
  -DENABLE_WEBRTC=OFF \
  -DENABLE_VLC=OFF \
  -DENABLE_VST=OFF \
  -DENABLE_SCRIPTING=OFF \
  -DENABLE_VIRTUALCAM=OFF \
  -DENABLE_NVAFX=OFF \
  -DENABLE_NVVFX=OFF \
  -DENABLE_JACK=OFF \
  -DENABLE_SNDIO=OFF \
  -DENABLE_COMPAT_UPDATES=OFF \
  -DENABLE_SERVICE_UPDATES=OFF \
  -DENABLE_PIPEWIRE=ON \
  -DENABLE_PULSEAUDIO=ON \
  -DENABLE_ALSA=ON \
  -DENABLE_WAYLAND=ON \
  -DENABLE_V4L2=ON \
  -DENABLE_UDEV=ON \
  -DENABLE_NVENC=ON \
  -DENABLE_QSV11=ON \
  -DENABLE_FREETYPE=ON \
  -DENABLE_NEW_MPEGTS_OUTPUT=OFF \
  -DENABLE_FFMPEG_LOGGING=OFF

echo "==> building"
cmake --build "$SRC/build" --parallel "$BUILD_JOBS"

echo "==> installing to staging prefix (DESTDIR, neutral compiled-in prefix)"
# The compiled-in install prefix must NOT exist at runtime, otherwise OBS
# scans it in addition to the relocatable path and loads every plugin twice.
rm -rf "$SRC/stage"
DESTDIR="$SRC/stage" cmake --install "$SRC/build"
PREFIX="$SRC/stage/nonexistent/moonclip-obs"

echo "==> staging to $OUT"
# Fedora installs into lib64; keep the same relative layout OBS expects
# (RUNPATH is $ORIGIN/../<libdir>).
LIBDIR="lib"
[ -d "$PREFIX/lib64" ] && LIBDIR="lib64"
# Legacy pre-patch bundle dir (upstream binary names).
rm -rf "$ROOT/src-tauri/binaries/$TRIPLE/obs"
rm -rf "$OUT"
mkdir -p "$OUT/bin" "$OUT/$LIBDIR" "$OUT/share"
# ALL of bin/: moonclip-engine + moonclip-mux (recording mux) +
# moonclip-nvenc-test (NVENC capability probe launched by obs-nvenc).
cp -a "$PREFIX/bin/." "$OUT/bin/"
cp -a "$PREFIX/$LIBDIR/." "$OUT/$LIBDIR/"
if [ -d "$PREFIX/share/obs" ]; then
  cp -a "$PREFIX/share/obs" "$OUT/share/obs"
fi
[ -d "$PREFIX/share/libobs" ] && cp -a "$PREFIX/share/libobs" "$OUT/share/libobs"

echo "==> verifying (isolated config, never touches ~/.config/obs-studio)"
XDG_CONFIG_HOME="$(mktemp -d)" "$OBS_BIN" --version | head -1
ls "$OUT/$LIBDIR/obs-plugins" | grep -E 'linux-pipewire|linux-pulseaudio|linux-capture|obs-ffmpeg|obs-nvenc|obs-x264|obs-websocket' || {
  echo "warning: expected plugins not found, listing:" >&2
  ls "$OUT/$LIBDIR/obs-plugins" >&2
}
for b in moonclip-engine moonclip-mux moonclip-nvenc-test; do
  [ -x "$OUT/bin/$b" ] || { echo "error: missing patched binary $b" >&2; exit 1; }
done
echo "patch=$PATCH_REV" > "$OUT/.moonclip-staged"
echo "OK: $OUT"
