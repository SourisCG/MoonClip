#!/usr/bin/env bash
# E2E: the poller starts the buffer when a REGISTERED app runs and stops it
# when the app closes. Uses Full screen mode (capture_mode=monitor) so no
# portal picker is needed, in an isolated XDG_DATA_HOME (real data untouched).
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TMP="$(mktemp -d /tmp/moonclip-poll-XXXXXX)"
GAME="moonclip-testgame"
fail=0
app_pid=""
game_pid=""

cleanup() {
  [ -n "$app_pid" ] && kill -- -"$app_pid" 2>/dev/null
  pkill -9 -f "engine/bin/mooncl[i]p-engine" 2>/dev/null
  [ -n "$game_pid" ] && kill "$game_pid" 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT

cd "$ROOT"

# 1. A real binary under a unique name stands in for the game (a script's
# /proc/exe points to the interpreter, which would not match).
cp /usr/bin/sleep "$TMP/$GAME"

# 2. Launch MoonClip isolated. The engine binary is invalid on purpose: the
# poller/detection runs for real but never opens the portal picker.
XDG_DATA_HOME="$TMP" MOONCLIP_OBS_BIN=/nonexistent/moonclip-engine \
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
sqlite3 "$DB" "UPDATE settings SET value='monitor' WHERE key='capture_mode';
               UPDATE settings SET value='$TMP/clips' WHERE key='clips_directory';"
sqlite3 "$DB" "INSERT INTO custom_apps
  (id, display_name, target_exe, match_strategy, clip_duration_seconds, icon_path, is_wine_proton)
  VALUES ('e2e-game', 'Test Game', '$GAME', 'exact_exe', NULL, NULL, 0);"

# 3. The registered app appears -> poller detects it and tries to record.
"$TMP/$GAME" 120 &
game_pid=$!
detected=0
for _ in $(seq 1 15); do
  if grep -q "game: Test Game" "$TMP/app.log"; then
    detected=1
    break
  fi
  sleep 1
done
if [ "$detected" = 1 ]; then
  echo "OK  registered app detected by the poller"
else
  echo "FAIL poller did not detect the registered app"
  tail -5 "$TMP/app.log"
  fail=1
fi

# 4. App closes -> poller clears the current game.
kill "$game_pid" 2>/dev/null
game_pid=""
cleared=0
for _ in $(seq 1 15); do
  if grep -q "game: none" "$TMP/app.log"; then
    cleared=1
    break
  fi
  sleep 1
done
if [ "$cleared" = 1 ]; then
  echo "OK  poller cleared the game when the app closed"
else
  echo "FAIL poller did not clear the game"
  fail=1
fi

if [ "$fail" -eq 0 ]; then
  echo "PASS game-poll-check"
fi
exit "$fail"
