# MoonClip

> Open-source, lightweight, local-first game clip recorder for Linux and Windows.
> Inspired by the idea of instant replay: press a hotkey, save the last seconds, edit lightly, share from your own storage.

[![License: GPL-3.0-only](https://img.shields.io/badge/License-GPL--3.0--only-blue.svg)](./LICENSE)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20Windows-lightgrey.svg)](https://github.com/SourisCG/MoonClip)
[![Built with Tauri v2](https://img.shields.io/badge/Tauri-v2-purple.svg)](https://tauri.app/)
[![Stack](https://img.shields.io/badge/stack-React_19_%2B_Rust-blue.svg)](./SPEC.md)
[![Status](https://img.shields.io/badge/status-Linux_Alpha_%7C_Windows_Next-yellow.svg)](./docs/ROADMAP_PHASES.md)

Website: [moonclip.souriscg.dev](https://moonclip.souriscg.dev/) · Full spec: [`SPEC.md`](./SPEC.md) · Docs: [`docs/`](./docs)

---

## Why MoonClip?

Clipping epic moments shouldn't require a heavy client, a cloud account, or a single-OS app.

Many popular clipping tools are closed-source, tied to their cloud, and resource-heavy, with limited support on Linux. **MoonClip takes a different approach:** capture what just happened while you game, with almost zero cost, and keep the files with you.

- **Press `F9` while playing** — get an `.mp4` of the last seconds in <1s.
- **Keep playing** — capture lives in RAM inside the embedded OBS child; the React UI stays hidden in tray.
- **Own your clips** — local files + SQLite + OS keyring. No central server. Share via *your* Google Drive or webhooks.

## How MoonClip is different

|  | Typical closed recorders | MoonClip |
|---|---|---|
| Cloud required | Often yes, account + upload | **No. Zero-cloud, local-first** |
| Linux support | Rare / limited | **Yes (X11 + Wayland via PipeWire portal, embedded OBS)** |
| Windows support | Yes | Yes (Windows 10 1903+ / 11, DXGI/WGC display capture, embedded OBS) |
| Idle footprint while gaming | Often heavy (bundled Chromium/Electron) | **<80 MB RAM, ~0% CPU, window hidden to tray** |
| Replay buffer | Sometimes | **Yes, RAM ring, no disk writes until save** |
| Audio tracks | Usually single mixed track | **3-track: MIX (game+mic) + game solo + mic solo** |
| Editor | Cloud / heavy | **Lightweight local: lossless trim, vertical 9:16, remix** |
| Sharing | Locked to vendor cloud link | **Your Drive (public link + clipboard (optional)) + Discord / YouTube / etc.** |
| Open source | Usually not | **Yes, GPL-3.0-only** |
| Languages | Usually EN only | **ES + EN from day one** |

## Features

### Working now (V3 — embedded OBS engine)

- **Replay buffer with global hotkey** — `F9` saves last N seconds (default 30s) from the embedded, isolated OBS Studio via `obs-cmd` (no re-encode; OBS flushes the RAM buffer to disk).
- **Mix-first 3-track audio** — Track 1 = game+mic mix (plays everywhere), Track 2 = game only, Track 3 = mic only for editing. Gain/mute 0-200% without touching what you hear.
- **Medal-style quality** — Moon preset cards (Low/Medium/New Moon/First Quarter/Waning Gibbous/Full Moon), 24/30/60/120/144 fps, H.264/H.265/AV1, GPU or CPU encoder, custom bitrate 3-100 Mbps, CBR ladder + Medal recommended ranges.
- **Anti-cheat safe** — display/window capture only (DXGI/WGC or portal); `game_capture` is never used, so Vanguard/VAC/FACEIT-style anti-cheats see a compositor capture, not a hook.
- **Zero mixing with your OBS** — MoonClip runs its own portable OBS copy with generated profile/scene and a private websocket; your `%APPDATA%\obs-studio` / `~/.config/obs-studio` is never read or written.
- **Gallery** — thumbnails + real durations, favorites, ghost-clip reconcile (auto-purge missing files), LRU prune.
- **Safe persistence** — SQLite with relative paths only (`base_dir + file_name`), secrets in OS keyring, never absolute paths in DB.
- **MoonClip UI** — frameless glass layout, pausable starfield (0 cost while gaming), bilingual ES/EN, tray with status + audio ding on save (audible in fullscreen).

### Coming soon

- **Game detection** — Steam (`SteamAppId` + `.acf`), Wine/Proton cmdline + blacklist, Minecraft/Prism/Bedrock, Heroic/Epic/Battle.net/Xbox, custom apps + process picker.
- **Lazy editor** — `React.lazy` ClipEditor + Wavesurfer Regions + dual waveforms, FFmpeg sidecar (lossless trim <1s, vertical HW, remix). Fully destroyed on close.
- **Sharing** — Drive PKCE + resumable upload + public link + clipboard, Discord webhook, Twitter/YouTube/TikTok flows.
- **Distribution** — `.exe/.msi/.AppImage/.deb/.rpm` from GitHub Releases on `v*` tags. Flathub / MS Store / WinGet later.

See [`docs/ROADMAP_PHASES.md`](./docs/ROADMAP_PHASES.md) and [`docs/PROGRESS.md`](./docs/PROGRESS.md) for acceptance checklists and build log.

## How it works

```
1. Play (MoonClip hidden to tray, OBS replay buffer in RAM, ~150 MB for 60s 1080p)
       ↓ press F9
2. Save (OBS flushes ring → .mp4 in the clips folder, indexed + thumb, ding plays)
       ↓
3. Browse (Gallery → favorite / preview / open externally)
       ↓
4. Edit & Share (trim lossless → vertical/remix if needed → upload to your Drive)
```

No pixels ever touch JS. React only sends `{start, end}` to Rust; Rust runs FFmpeg CLI.

## Quick Start

### Prerequisites

- Node 20 + `pnpm` (`corepack enable pnpm` or `npm i -g pnpm`)
- Rust stable + system webview deps:
  ```bash
  # Fedora / RHEL
  sudo dnf install webkit2gtk4.1-devel libappindicator-gtk3-devel librsvg2-devel

  # Ubuntu / Debian
  sudo apt-get install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
  ```
- Linux screen capture: the embedded OBS asks the XDG Portal for the screen the first time ("Share screen" + Remember). No caps, no `pkexec` prompts.
- Windows: 10 version 1903 (build 18362)+ or 11. MSVC toolchain.
- Sidecars are fetched once into `src-tauri/binaries/`:
  ```powershell
  pwsh build-aux/fetch-obs.ps1      # OBS 32.2.2 + obs-cmd 1.0.2 (pinned, sha256)
  pwsh build-aux/fetch-ffmpeg.ps1   # editor/probe FFmpeg (pinned)
  ```

### Develop

```bash
pnpm install
pnpm tauri dev
```

### Build

```bash
pnpm build
# Tauri bundle -> src-tauri/target/release/bundle/
```

Linux local install (fully embedded: OBS + obs-cmd + static ffmpeg in the same package):

```bash
bash build-aux/build-obs.sh    # compile the pinned OBS once (cached, ~15 min)
bash build-aux/fetch-ffmpeg.sh # pinned static ffmpeg sidecar
pnpm tauri:build:linux         # RPM with sidecars under /usr/lib/MoonClip/binaries/
pnpm app:install               # replace install + taskbar association
```

> Windows SmartScreen (early builds): app is unsigned OSS yet. Click `More info → Run anyway`. Builds are auditable via GitHub Actions. Signing via Store/MSIX comes after Phase 7.

## Usage

- `F9` (default, changeable in Settings → Clip hotkey) — save clip (global, works in fullscreen exclusive).
- Tray icon — buffer status (active/idle), show/hide, quit. Main window minimizes to tray while gaming.
- Settings — buffer length, resolution/FPS/codec/encoder (Medal-style presets + custom), monitor, `gain_game/gain_mic` + mutes, base folder, locale ES/EN, hotkey (F9 default), isolated OBS engine panel (status + repair).
- Clips live in `~/Videos/MoonClip` by default. DB stores only `file_name`, resolved at runtime as `base_dir.join(file_name)` — move the folder freely.

## Privacy: local-first, zero-cloud

- No central server, no telemetry, no account.
- Clips + SQLite stay on disk. Tokens (e.g. `google_drive_refresh_token`) stay in OS keyring (libsecret / Credential Manager / Keychain).
- Uploads go client → service directly (Drive API, Discord webhook). You revoke them where you created them.

## Tech stack (brief)

> Gamers can skip this. Contributors: start at [`SPEC.md`](./SPEC.md) + [`docs/01_ARCHITECTURE.md`](./docs/01_ARCHITECTURE.md).

- **App:** Tauri v2 + React 19 + TypeScript + Vite + Tailwind v3 + `react-i18next` + Wavesurfer v7 + Lucide
- **Backend:** Rust (tokio, serde, rusqlite, keyring, rodio, uuid, dirs, nix/image on Linux)
- **Capture:** embedded, isolated OBS Studio (Windows: DXGI/WGC display capture + WASAPI; Linux: PipeWire portal + PulseAudio), driven by the bundled `obs-cmd` over a private obs-websocket. Display sources only — no game hooks.
- **Sidecars:** `obs` + `obs-cmd` + static `ffmpeg` in `src-tauri/binaries/` (pinned, sha256-verified)
- **Rules:** HW-encode first, lossless-cut by default, lazy editor, relative paths, zero `cfg(target_os)` outside `os/`, IPC wire keys always camelCase

```
moonclip/
├── docs/          # EN technical spec (01-09 + ROADMAP + PROGRESS + THIRD_PARTY)
├── SPEC.md        # Index + acceptance map
├── src-tauri/src/os/  # ALL platform code (linux/ + windows/ behind traits)
├── src-tauri/src/storage/ editor/ uploader/ detector/
└── src/components/ (starfield / gallery / editor(lazy) / settings)
```

## License

Copyright (C) 2026 SourisCG

This program is free software: you can redistribute it and/or modify it under the terms of the **GNU General Public License version 3 only**, as published by the Free Software Foundation. See [LICENSE](./LICENSE).
