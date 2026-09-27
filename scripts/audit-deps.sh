#!/usr/bin/env bash
# audit-deps.sh — regenerate the dependency table in docs/10_DEPENDENCIES.md.
#
# Scans the release app binary, the embedded capture engine, its plugins and
# the bundled ffmpeg with ldd, then maps every soname to the system package
# that provides it:
#   - resolved libs  -> local package database (rpm -qf / dpkg -S / pacman -Qo)
#   - missing libs   -> the distro package search (repoquery / apt-file / pacman -F)
# Re-run it in the packaging phase or after any engine update; it only
# replaces the section between the BEGIN/END markers in the doc.
#
# Usage: scripts/audit-deps.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRIPLE="${MOONCLIP_TRIPLE:-$(rustc -vV 2>/dev/null | awk '/^host:/{print $2}')}"
TRIPLE="${TRIPLE:-x86_64-unknown-linux-gnu}"
BINDIR="$ROOT/src-tauri/binaries/$TRIPLE"
DOC="${MOONCLIP_DEP_DOC:-$ROOT/docs/10_DEPENDENCIES.md}"

if [[ ! -f "$DOC" ]]; then
  echo "error: $DOC not found (markers live there)" >&2
  exit 1
fi
if ! grep -q "BEGIN GENERATED: audit-deps.sh" "$DOC"; then
  echo "error: $DOC has no '<!-- BEGIN GENERATED: audit-deps.sh -->' marker" >&2
  exit 1
fi

targets=()
add() { [[ -f "$1" ]] && targets+=("$1"); }
add "$ROOT/src-tauri/target/release/moonclip"
add "$BINDIR/engine/bin/moonclip-engine"
add "$BINDIR/engine/bin/moonclip-mux"
add "$BINDIR/ffmpeg-$TRIPLE"
while IFS= read -r -d '' f; do targets+=("$f"); done \
  < <(find "$BINDIR/engine/lib64/engine-plugins" -name '*.so' -print0 2>/dev/null)

if [[ ${#targets[@]} -eq 0 ]]; then
  echo "error: no binaries found under $BINDIR (build/stage first)" >&2
  exit 1
fi

label() {
  case "$(basename "$1")" in
    moonclip) echo "app" ;;
    moonclip-engine) echo "engine" ;;
    moonclip-mux) echo "mux" ;;
    ffmpeg-*) echo "ffmpeg" ;;
    *.so) echo "plugin" ;;
    *) echo "$(basename "$1")" ;;
  esac
}

declare -A needed=()   # soname -> "app engine plugin" (unique labels)
declare -A paths=()    # soname -> resolved path
declare -A missing=()  # soname -> 1

for t in "${targets[@]}"; do
  lbl="$(label "$t")"
  while read -r name arrow path _rest; do
    [[ -n "${name:-}" ]] || continue
    # Skip the interpreter and the kernel vdso: not packages.
    [[ "$name" == /* ]] && continue
    case "$name" in linux-vdso*|ld-linux*) continue ;; esac
    if [[ "${arrow:-}" == "=>" && "${path:-}" == "not" ]]; then
      missing["$name"]=1
    elif [[ -n "${path:-}" ]]; then
      paths["$name"]="$path"
    fi
    case " ${needed[$name]:-} " in
      *" $lbl "*) ;;
      *) needed["$name"]="${needed[$name]:+${needed[$name]} }$lbl" ;;
    esac
  done < <(ldd "$t" 2>/dev/null)
done

# Local package database lookup for a resolved file path.
pkg_for_path() {
  local p="$1"
  if command -v rpm >/dev/null 2>&1; then
    rpm -qf --qf '%{NAME}' "$p" 2>/dev/null && return 0
  elif command -v dpkg-query >/dev/null 2>&1; then
    dpkg-query -S "$p" 2>/dev/null | head -1 | cut -d: -f1 && return 0
  elif command -v pacman >/dev/null 2>&1; then
    pacman -Qo "$p" 2>/dev/null | awk '{print $(NF-1)}' && return 0
  fi
  return 1
}

# Distro package search for libs that are not installed.
pkg_for_missing() {
  local so="$1"
  if command -v dnf >/dev/null 2>&1; then
    dnf -q repoquery --whatprovides "*/$so" --queryformat '%{name} [%{repoid}]\n' 2>/dev/null \
      | grep -E '\[(fedora|updates|rpmfusion[^]]*)\]' | head -1 | cut -d' ' -f1 \
      || dnf -q repoquery --whatprovides "*/$so" --queryformat '%{name}\n' 2>/dev/null | head -1
  elif command -v apt-file >/dev/null 2>&1; then
    apt-file -q search "/$so" 2>/dev/null | head -1 | cut -d: -f1
  elif command -v pacman >/dev/null 2>&1; then
    pacman -Fq "$so" 2>/dev/null | head -1
  fi
}

tmp="$(mktemp)"
{
  echo "<!-- BEGIN GENERATED: audit-deps.sh -->"
  echo "_Generated $(date -u '+%Y-%m-%d %H:%M UTC') from the app, engine, mux, engine plugins and ffmpeg._"
  echo
  echo "| soname | Provided by | Needed by |"
  echo "|---|---|---|"
  for so in $(printf '%s\n' "${!needed[@]}" | sort); do
    if [[ -n "${missing[$so]:-}" ]]; then
      pkg="$(pkg_for_missing "$so" || true)"
      if [[ -n "$pkg" ]]; then prov="**$pkg** (not installed)"; else prov="**MISSING**"; fi
    elif [[ "${paths[$so]}" == "$BINDIR"/* ]]; then
      # Shipped inside the bundle (libobs and friends), not a system package.
      prov="bundled"
    else
      prov="$(pkg_for_path "${paths[$so]}")" || prov="?"
    fi
    printf '| `%s` | %s | %s |\n' "$so" "$prov" "${needed[$so]}"
  done
  echo
  echo "<!-- END GENERATED: audit-deps.sh -->"
} > "$tmp"

out="$(mktemp)"
awk -v table="$tmp" '
  /<!-- BEGIN GENERATED: audit-deps.sh -->/ {
    while ((getline line < table) > 0) print line
    close(table)
    skip = 1
    next
  }
  /<!-- END GENERATED: audit-deps.sh -->/ { skip = 0; next }
  !skip { print }
' "$DOC" > "$out"
mv "$out" "$DOC"
rm -f "$tmp"

echo "updated $DOC: $(printf '%s\n' "${!needed[@]}" | wc -l) sonames, ${#missing[@]} missing"
