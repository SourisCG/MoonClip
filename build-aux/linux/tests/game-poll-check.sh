#!/usr/bin/env bash
# E2E: the autopilot checker lists desktop windows and starts/stops by window
# title. Uses MOONCLIP_FAKE_WINDOWS_FILE (debug builds) so no real window is
# needed, in an isolated XDG_DATA_HOME (real data untouched).
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TMP="$(mktemp -d /tmp/moonclip-poll-XXXXXX)"
fail=0
app_pid=""

cleanup() {
  [ -n "$app_pid" ] && kill -- -"$app_pid" 2>/dev/null
  pkill -9 -f "engine/bin/mooncl[i]p-engine" 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT

cd "$ROOT"

WINDOWS="$TMP/windows.json"
echo '[]' >"$WINDOWS"

# Launch MoonClip isolated. The engine binary is invalid on purpose: the
# checker/detection runs for real but never opens a picker.
XDG_DATA_HOME="$TMP" MOONCLIP_OBS_BIN=/nonexistent/moonclip-engine \
  MOONCLIP_FAKE_WINDOWS_FILE="$WINDOWS" \
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
sqlite3 "$DB" "UPDATE settings SET value='$TMP/clips' WHERE key='clips_directory';"
sqlite3 "$DB" "INSERT INTO custom_apps
  (id, input_name, input_kind, display_name, window_title, window_app_id,
   target_exe, match_strategy, clip_duration_seconds, icon_path, is_wine_proton, source_uuid)
  VALUES ('e2e-game', 'Test Game', 'window', 'Test Game', 'Moonclip Test Game',
          'moonclip-testgame', '', 'window', NULL, NULL, 0, 'e2e-uuid-0001');"

# The registered window appears -> the checker sees it.
echo '[{"title":"Moonclip Test Game"}]' >"$WINDOWS"
detected=0
for _ in $(seq 1 15); do
  if grep -q "game: Test Game" "$TMP/app.log"; then
    detected=1
    break
  fi
  sleep 1
done
if [ "$detected" = 1 ]; then
  echo "OK  registered game window detected by the checker"
else
  echo "FAIL checker did not detect the registered window"
  tail -5 "$TMP/app.log"
  fail=1
fi

# Window closes -> the checker clears the current game.
echo '[]' >"$WINDOWS"
cleared=0
for _ in $(seq 1 15); do
  if grep -q "game: none" "$TMP/app.log"; then
    cleared=1
    break
  fi
  sleep 1
done
if [ "$cleared" = 1 ]; then
  echo "OK  checker cleared the game when the window closed"
else
  echo "FAIL checker did not clear the game"
  fail=1
fi

if [ "$fail" -eq 0 ]; then
  echo "PASS game-poll-check"
fi
exit "$fail"
