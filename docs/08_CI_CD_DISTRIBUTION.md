# 08 — CI/CD & Distribution (MVP-first, signing later)

Developer OS: Fedora. Targets: WinGet/MS Store (later), Flathub (later), direct `.exe/.msi/.rpm/.deb/.AppImage` now.

## 1. Tauri bundles (MVP)

```json
{
  "bundle": {
    "active": true,
    "targets": ["nsis", "msi", "appimage", "deb", "rpm"],
    "identifier": "dev.souriscg.moonclip",
    "windows": { "nsis": { "oneClick": false, "perMachine": false } },
    "linux": {
      "deb": { "depends": ["libwebkit2gtk-4.1-0", "libayatana-appindicator3-1"] },
      "rpm": { "depends": ["webkit2gtk4.1", "libappindicator-gtk3"] }
    }
  }
}
```

Add `"msix"` only when Partner Center flow starts. Current `tauri.conf.json` has `targets: "all"` (scaffold default) — narrow it in Phase 7.

Minimum supported Windows version: **Windows 10 version 1903 (build 18362)+**
(WGC capture floor; Windows 11 supported). NSIS/MSI metadata must reflect
this floor so the installer refuses older builds instead of installing a
build whose capture backend cannot initialize.

Sidecars in `src-tauri/binaries/`: `ffmpeg-x86_64-pc-windows-msvc.exe`, `ffmpeg-x86_64-unknown-linux-gnu` (static: BtbN / johnvansickle musl). The
Windows ffmpeg ships via `bundle.resources` (raw binary pipes — the
shell-plugin sidecar API is line-events only and cannot carry them), added
through a **per-OS CI config overlay** (`tauri build --config ...`), never
in the base `tauri.conf.json`, so Linux bundles never carry the `.exe`.
Plus `moonclip-gsr/gpu-screen-recorder` (+ `gsr-kms-server`) for Linux targets,
built in CI from the pinned snapshot in `docs/THIRD_PARTY.md`. Full ship matrix
(user installs nothing extra) is defined there.
Windows agent: MSVC toolchain + `pnpm install` (pnpm via corepack/npm);
`host_triple()` in `sidecar.rs` already emits `x86_64-pc-windows-msvc`.
No GSR build needed on Windows (native WGC backend).

## 2. GitHub Actions (`/.github/workflows/release.yml`, Phase 7)

Trigger on `v*` tags + manual dispatch. Matrix `windows-latest` + `ubuntu-22.04`, `tauri-apps/tauri-action@v0`:

```yaml
jobs:
  build:
    strategy: { fail-fast: false, matrix: { include: [{os: windows-latest}, {os: ubuntu-22.04}] } }
    runs-on: ${{ matrix.os }}
    permissions: { contents: write }
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 20, cache: npm } # pnpm: use pnpm/action-setup + cache pnpm
      - uses: dtolnay/rust-toolchain@stable
      - uses: swatinem/rust-cache@v2
        with: { workspaces: './src-tauri -> target' }
      - if: matrix.os == 'ubuntu-22.04'
        run: sudo apt-get update && sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev rpm
      - run: pnpm install # repo uses pnpm (pnpm-lock.yaml); do NOT use npm ci here
      - uses: tauri-apps/tauri-action@v0
        env: { GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }} }
        with:
          tagName: ${{ github.ref_name }}
          releaseName: 'MoonClip ${{ github.ref_name }}'
          releaseBody: |
            ### Install
            * **Windows:** `.exe` or `.msi`. Unsigned yet: SmartScreen → More info → Run anyway.
            * **Fedora/RHEL:** `.rpm` (`sudo dnf install ./MoonClip-*.rpm`).
            * **Generic Linux:** `.AppImage` (`chmod +x`).
            * **Ubuntu/Debian:** `.deb`.
```

Note: template uses `cache: 'npm'` by default; switch to pnpm setup when writing the real workflow.

Artifacts per tag: `_x64-setup.exe`, `_x64_en-US.msi`, `_amd64.AppImage`, `-1.x86_64.rpm`, `_amd64.deb`.

## 3. Signing / stores (deferred, documented)

- **Windows now:** unsigned NSIS `.exe` + README SmartScreen note ("More info → Run anyway", auditable Actions). Reputation accrues faster with NSIS installer vs loose portable.
- **Later:** SignPath Foundation (free for OSS) or Azure Trusted Signing (~$10/mo). Best trick: MS Store `.msix` → Microsoft signs with trusted root → SmartScreen clean + auto-available in WinGet `msstore` source (`winget install "MoonClip"`). Needs $19 one-time Partner Center account. WinGet community (`winget-pkgs`) via `winget-releaser` with `WINGET_TOKEN` after signing.
- **Linux later:** Flathub manifest (`com.souriscg.moonclip.yml`, runtime `org.freedesktop 24.08`, `--device=dri --socket=wayland --socket=fallback-x11 --socket=pulseaudio --talk-name=org.freedesktop.portal.* --filesystem=xdg-videos`). Flathub builds from source/manifest, not just attached `.flatpak`.
- WinGet does NOT sign binaries; it hash-checks your Release asset.

## 4. README note (Phase 7)

> Windows SmartScreen: early OSS build, no paid Authenticode yet. Click More info → Run anyway. Verify source + transparent Actions build.
