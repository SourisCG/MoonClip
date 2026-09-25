#!/usr/bin/env bash
# Verify the staged engine bundle follows the MoonClip identity: patched
# binaries exist and no runtime file name leaks the upstream product name.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
OUT="$ROOT/src-tauri/binaries/x86_64-unknown-linux-gnu/engine"
fail=0

for b in moonclip-engine moonclip-mux moonclip-nvenc-test; do
  if [ -x "$OUT/bin/$b" ]; then
    echo "OK  bin/$b"
  else
    echo "FAIL missing bin/$b"
    fail=1
  fi
done

# Process surface (ps/comm/Discord): only bin/ matters, and no name may
# contain the upstream product string.
leaks="$(find "$OUT/bin" -maxdepth 1 -iname '*obs*' 2>/dev/null)"
if [ -n "$leaks" ]; then
  echo "FAIL bin/ still contains upstream-named files:"
  echo "$leaks"
  fail=1
else
  echo "OK  bin/ has no upstream-named files"
fi

if [ "$fail" -eq 0 ]; then
  echo "PASS verify-engine-bundle"
fi
exit "$fail"
