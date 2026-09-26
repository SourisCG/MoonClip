#!/usr/bin/env bash
# E2E: a *registered* game auto-starts the buffer (silent X11 window capture)
# and auto-stops when it disappears. No game hooks: a real X11 window (zenity)
# is registered exactly like the user would, in an isolated XDG_DATA_HOME so
# the real MoonClip DB/config are untouched.
#
# Usage: build-aux/linux/tests/auto-buffer-check.sh
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TMP="$(mktemp -d /tmp/moonclip-auto-XXXXXX)"
GAME="MoonClip Test Game"
SECS=20
fail=0
app_pid=""
win_pid=""

cleanup() {
  [ -n "$app_pid" ] && kill -- -"$app_pid" 2>/dev/null
  pkill -9 -f "engine/bin/mooncl[i]p-engine" 2>/dev/null
  [ -n "$win_pid" ] && kill "$win_pid" 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT

cd "$ROOT"

# 1. A real X11 window stands in for the game (zenity forced to GDK x11).
GDK_BACKEND=x11 setsid zenity --info --title "MoonClip Test Window" \
  --text "auto-buffer test" >/dev/null 2>&1 &
win_pid=$!
sleep 2
win=""
for id in $(xprop -root _NET_CLIENT_LIST 2>/dev/null | tr ',' '\n' | grep -o '0x[0-9a-f]*'); do
  info="$(xprop -id "$id" _NET_WM_PID WM_CLASS _NET_WM_NAME 2>/dev/null | tr '\n' ' ')"
  case "$info" in
    *"MoonClip Test Window"*) win="$id"; break ;;
  esac
done
if [ -z "$win" ]; then
  echo "FAIL could not find the test window (is XWayland up?)"
  exit 1
fi
wid=$(printf '%d' "$win")
match=$(printf '%b' "$wid\\r\\nMoonClip Test Window\\r\\nzenity")

# 2. Launch MoonClip isolated, with the debug-only synthetic candidate.
XDG_DATA_HOME="$TMP" \
  MOONCLIP_FAKE_GAME="$GAME" \
  MOONCLIP_FAKE_GAME_SECS="$SECS" \
  MOONCLIP_FAKE_WINDOW_MATCH="$match" \
  setsid pnpm tauri:dev >"$TMP/app.log" 2>&1 &
app_pid=$!

DB="$TMP/dev.souriscg.moonclip/moonclip.db"
for _ in $(seq 1 90); do
  [ -f "$DB" ] && break
  sleep 1
done
if [ ! -f "$DB" ]; then
  echo "FAIL MoonClip DB never appeared"
  tail -5 "$TMP/app.log"
  exit 1
fi
sleep 3

# 3. Register the game once (same shape the Games UI will insert).
sqlite3 "$DB" "INSERT INTO custom_apps
  (id, display_name, target_exe, match_strategy, clip_duration_seconds, icon_path,
   is_wine_proton, capture_mode, auto_buffer)
  VALUES ('e2e-game', '$GAME', '$GAME', 'exact_exe', NULL, NULL, 0, 'window', 1);"

# 4. Auto buffer starts (3 s debounce).
started=0
for _ in $(seq 1 15); do
  if pgrep -f "engine/bin/mooncl[i]p-engine" >/dev/null; then
    started=1
    break
  fi
  sleep 1
done
if [ "$started" = 1 ]; then
  echo "OK  auto buffer started"
else
  echo "FAIL auto buffer did not start"
  grep -E 'auto-buffer|engine' "$TMP/app.log" | tail -5
  fail=1
fi

# 5. Window capture: the generated collection must use xcomposite_input.
COLL="$TMP/MoonClip/engine/config/moonclip-engine/basic/scenes/MoonClip.json"
if [ -f "$COLL" ] && grep -q 'xcomposite_input' "$COLL"; then
  echo "OK  collection captures the X11 window (xcomposite_input)"
else
  echo "FAIL collection missing xcomposite_input"
  fail=1
fi

# 6. Game gone (SECS) -> auto stop (7 s debounce).
sleep $((SECS + 12))
if pgrep -f "engine/bin/mooncl[i]p-engine" >/dev/null; then
  echo "FAIL auto buffer did not stop after the game closed"
  fail=1
else
  echo "OK  auto buffer stopped with the game"
fi

if [ "$fail" -eq 0 ]; then
  echo "PASS auto-buffer-check"
fi
exit "$fail"
