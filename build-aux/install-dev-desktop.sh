#!/usr/bin/env bash
# Install dev .desktop so KDE/GNOME Wayland matches the window (appId
# dev.souriscg.moonclip from tauri.conf.json > app.identifier + enableGTKAppId)
# to the taskbar icon instead of showing the generic Wayland placeholder.
# Tray is unaffected: it uses icons/tray-icon.png via TrayIconBuilder in lib.rs.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEMPLATE="$ROOT/build-aux/dev.souriscg.moonclip.desktop.template"
OUT="$HOME/.local/share/applications/dev.souriscg.moonclip.desktop"

echo "==> generating $OUT from $TEMPLATE"
mkdir -p "$(dirname "$OUT")"
sed "s#__REPO__#${ROOT}#g" "$TEMPLATE" > "$OUT.tmp" && mv "$OUT.tmp" "$OUT"

desktop-file-validate "$OUT" && echo "==> desktop-file-validate OK"
update-desktop-database "$(dirname "$OUT")" 2>/dev/null || true
if command -v kbuildsycoca6 >/dev/null; then
  kbuildsycoca6 >/dev/null 2>&1 || true
  echo "==> kbuildsycoca6 done"
fi

echo "OK: $OUT"
echo "Next: unpin any old MoonClip task-manager pin, fully quit MoonClip (tray > Quit), then: pnpm tauri:dev"
echo "If the generic icon persists after relaunch: log out/in once so KWin reloads the appId map."
