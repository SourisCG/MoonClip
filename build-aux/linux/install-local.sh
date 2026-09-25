#!/usr/bin/env bash
# Local install/upgrade of the embedded build on THIS machine.
# Package install + KDE/Wayland taskbar association.
# Re-run after each new build:  pnpm tauri:build:linux && pnpm app:install
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RPM="$(ls -t "$ROOT"/src-tauri/target/release/bundle/rpm/*.rpm 2>/dev/null | head -1)"

if [ -z "$RPM" ]; then
  echo "error: no RPM found. Run first: pnpm tauri:build:linux" >&2
  exit 1
fi

echo "==> installing $RPM"
pkexec rpm -Uvh --replacepkgs "$RPM"

# KDE/Wayland matches the window appId (`dev.souriscg.moonclip`) against a
# .desktop file with that exact name. The bundled file is MoonClip.desktop, so
# install a hidden alias (NoDisplay keeps a single visible menu entry).
ALIAS="$(mktemp /tmp/moonclip-alias-XXXXXX.desktop)"
trap 'rm -f "$ALIAS"' EXIT
cat > "$ALIAS" <<'EOF'
[Desktop Entry]
Type=Application
Name=MoonClip
Comment=MoonClip window association (KDE/Wayland appId match)
NoDisplay=true
Exec=moonclip
Icon=moonclip
StartupWMClass=dev.souriscg.moonclip
EOF
pkexec install -m 644 "$ALIAS" /usr/share/applications/dev.souriscg.moonclip.desktop
pkexec update-desktop-database /usr/share/applications 2>/dev/null || true
if command -v kbuildsycoca6 >/dev/null; then
  kbuildsycoca6 >/dev/null 2>&1 || true
fi

echo "OK: installed and taskbar association present"
echo "Launch from the app menu (MoonClip)."
