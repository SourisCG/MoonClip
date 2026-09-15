#!/usr/bin/env bash
# Build gpu-screen-recorder from pinned source into src-tauri/binaries/<triple>/.
# Embedded sidecar: end users never install GSR separately (see docs/THIRD_PARTY.md).
# Arch-safe: always compiles natively on the host; output tagged with rustc triple.
set -euo pipefail

# Pinned source: GitHub mirror (upstream git is bot-walled). Base = upstream 5.15.1.
GSR_REPO="https://github.com/antonlobanovskiy/gpu-screen-recorder"
GSR_COMMIT="a2cfc66419fd7c814ab46e330592c983404b059b"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
OUT="$ROOT/src-tauri/binaries/$TRIPLE"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

MESON_PY="${MESON_PY:-}"
if [ -z "$MESON_PY" ]; then
  if command -v meson >/dev/null; then
    MESON_PY="$(command -v meson)"
  else
    echo "error: meson not found. Install it (dnf install meson) or set MESON_PY=/path/to/meson.py" >&2
    exit 1
  fi
fi

echo "==> fetching $GSR_REPO @ $GSR_COMMIT"
git clone --quiet "$GSR_REPO" "$WORK/gsr"
git -C "$WORK/gsr" fetch --quiet --depth 1 origin "$GSR_COMMIT"
git -C "$WORK/gsr" checkout --quiet "$GSR_COMMIT"

echo "==> configuring (release, no caps/systemd, triple $TRIPLE)"
python3 "$MESON_PY" setup "$WORK/gsr/build" "$WORK/gsr" \
  --prefix=/usr \
  --buildtype=release \
  -Dcapabilities=false \
  -Dsystemd=false \
  -Dnvidia_suspend_fix=false \
  -Dstrip=true

echo "==> building"
ninja -C "$WORK/gsr/build" gpu-screen-recorder gsr-kms-server

echo "==> staging to $OUT"
mkdir -p "$OUT"
cp "$WORK/gsr/build/gpu-screen-recorder" "$OUT/"
cp "$WORK/gsr/build/gsr-kms-server" "$OUT/"
strip "$OUT/gpu-screen-recorder" "$OUT/gsr-kms-server" || true

echo "==> verifying"
file "$OUT/gpu-screen-recorder" | grep -qi "${TRIPLE%%-*}.*64\|x86-64" || {
  echo "warning: unexpected arch, check manually:"; file "$OUT"/*;
}
"$OUT/gpu-screen-recorder" --version
echo "OK: $OUT"
