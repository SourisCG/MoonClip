#!/usr/bin/env bash
# Live stealth check (Linux). Run while the replay buffer is active:
#
#   MOONCLIP_E2E_AUTOSTART=1 pnpm tauri:dev &
#   build-aux/linux/tests/stealth-live-check.sh
#
# Asserts that the engine process surface (processes, threads, unix sockets)
# never exposes the upstream product identity. The user's own OBS (system
# package) is deliberately ignored: only our bundle PIDs are inspected.
set -uo pipefail

TRIPLE="x86_64-unknown-linux-gnu"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
BUNDLE="binaries/$TRIPLE/engine/bin"
fail=0

pids=$(pgrep -f "$BUNDLE/moonclip-engine" || true)
mux=$(pgrep -f "$BUNDLE/moonclip-mux" || true)
if [ -z "$pids" ]; then
  echo "FAIL no moonclip-engine process found (start the buffer first)"
  exit 1
fi
echo "OK  engine pids: $pids"
if [ -n "$mux" ]; then
  echo "OK  mux pids: $mux"
else
  echo "INFO moonclip-mux idle (the replay buffer keeps packets in RAM; the"
  echo "     mux is spawned at save time — see save_replay.py)"
fi

check_pid() {
  local pid="$1" what="$2"
  local comm cmd exe name
  comm="$(cat "/proc/$pid/comm" 2>/dev/null || true)"
  cmd="$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null || true)"
  exe="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
  case "$comm$cmd$exe" in
    *[Oo][Bb][Ss]*)
      echo "FAIL $what pid=$pid leaks upstream identity: comm=$comm exe=$exe"
      fail=1
      ;;
    *)
      echo "OK  $what pid=$pid ($comm)"
      ;;
  esac
  for t in /proc/"$pid"/task/*/comm; do
    [ -r "$t" ] || continue
    name="$(cat "$t" 2>/dev/null || true)"
    case "$name" in
      *[Oo][Bb][Ss]*)
        echo "FAIL thread $t leaks upstream identity: $name"
        fail=1
        ;;
    esac
  done
}

for pid in $pids; do check_pid "$pid" engine; done
for pid in $mux; do check_pid "$pid" mux; done

# Abstract unix sockets: the upstream single-instance fingerprint lived at
# @/com/obsproject with the owning PID embedded in its name; our build must
# never register it (patch 0002).
unix_socks="$(grep 'com/obsproject' /proc/net/unix 2>/dev/null || true)"
hit=0
for pid in $pids $mux; do
  if printf '%s\n' "$unix_socks" | grep -q "com/obsproject $pid "; then
    hit=1
  fi
done
if [ "$hit" -eq 1 ]; then
  echo "FAIL engine owns an upstream-named unix socket"
  fail=1
else
  echo "OK  no upstream-named unix socket owned by the engine"
fi

# Pulse/PipeWire client identity: every audio client owned by the engine must
# carry a neutral application name/icon (patched in step 3.4).
if command -v pactl >/dev/null 2>&1 && pactl info >/dev/null 2>&1; then
  clients="$(pactl list clients 2>/dev/null || true)"
  hits=0
  for pid in $pids; do
    block="$(printf '%s\n' "$clients" | awk -v RS='Client #' -v pid="$pid" \
      '$0 ~ ("application.process.id = \"" pid "\"") { print }')"
    if printf '%s\n' "$block" | grep -qi 'obs'; then
      echo "FAIL engine pulse client (pid $pid) leaks upstream identity"
      hits=1
      fail=1
    fi
  done
  [ "$hits" -eq 0 ] && echo "OK  engine pulse clients carry no upstream identity"
fi

# Portal identity: Qt's registry warning ("Connection already associated with
# an application ID") must be gone thanks to QT_NO_XDG_DESKTOP_PORTAL.
cfg_root="${XDG_DATA_HOME:-$HOME/.local/share}/MoonClip/obs/config"
log="$(ls -t "$cfg_root"/obs-studio/logs/*.txt 2>/dev/null | head -1 || true)"
if [ -n "$log" ]; then
  if grep -q 'Could not register app ID' "$log"; then
    echo "FAIL portal app-id warning present in $(basename "$log")"
    fail=1
  else
    echo "OK  no portal app-id warning in the newest engine log"
  fi
fi

if [ "$fail" -eq 0 ]; then
  echo "PASS stealth-live-check"
fi
exit "$fail"