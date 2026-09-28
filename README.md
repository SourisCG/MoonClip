# MoonClip

> Open-source, lightweight, local-first game clip recorder for Linux and Windows.
> Inspired by the idea of instant replay: press a hotkey, save the last seconds, edit lightly, share from your own storage.

[![License: GPL-3.0-only](https://img.shields.io/badge/License-GPL--3.0--only-blue.svg)](./LICENSE)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20Windows-lightgrey.svg)](https://github.com/SourisCG/MoonClip)
[![Built with Tauri v2](https://img.shields.io/badge/Tauri-v2-purple.svg)](https://tauri.app/)
[![Stack](https://img.shields.io/badge/stack-React_19_%2B_Rust-blue.svg)](./SPEC.md)
[![Status](https://img.shields.io/badge/status-Linux_alpha_%7C_Windows_next-yellow.svg)](./docs/ROADMAP_PHASES.md)

Website: [moonclip.souriscg.dev](https://moonclip.souriscg.dev/) · Full spec: [`SPEC.md`](./SPEC.md) · Docs: [`docs/`](./docs)

---

## Why MoonClip?

Clipping epic moments shouldn't require a heavy client, a cloud account, or a single-OS app.

Many popular clipping tools are closed-source, tied to their cloud, and resource-heavy, with limited support on Linux. **MoonClip takes a different approach:** capture what just happened while you game, with almost zero cost, and keep the files with you.

- **Press `F9` while playing** — get an `.mp4` of the last seconds in <1s.
- **Keep playing** — capture lives in RAM inside the embedded OBS child; the React UI stays hidden in tray.
- **Own your clips** — local files + SQLite + OS keyring. No central server. Share via *your* Google Drive, YouTube or Discord.

## Status (2026-09-28)

| Phase | Scope | Status |
|---|---|---|
| 0–2 | Architecture, UI base, storage & security | done |
| 3 | Capture engine (embedded isolated OBS, 3-track audio) | done (Linux); Windows in-game pass pending |
| 4 | Game detection (registered windows + autopilot) | done |
| 5 | Editor (quick trim E1 + advanced editor E2–E6) | done |
| 6 | Sharing (Drive + restore, YouTube, Discord, TikTok) | code done; live passes + Twitter/X pending |
| 7 | CI/CD packaging | pending |
| 8 | Distribution & dependencies | in progress |

Details and acceptance checklists: [`docs/ROADMAP_PHASES.md`](./docs/ROADMAP_PHASES.md) and [`docs/PROGRESS.md`](./docs/PROGRESS.md).

## How MoonClip is different

|  | Typical closed recorders | MoonClip |
|---|---|---|
| Cloud required | Often yes, account + upload | **No. Zero-account, local-first** |
| Linux support | Rare / limited | **Yes (Wayland via PipeWire portal, embedded OBS)** |
| Windows support | Yes | Yes (Windows 10 1903+ / 11, DXGI/WGC display capture, embedded OBS) |
| Idle footprint while gaming | Often heavy (bundled Chromium/Electron) | **<80 MB RAM, ~0% CPU, window hidden to tray** |
| Replay buffer | Sometimes | **Yes, RAM ring, no disk writes until save** |
| Audio tracks | Usually single mixed track | **3-track: MIX (game+mic) + game solo + mic solo** |
| Audio controls | Basic slider | **OBS-style mixer: 0–100 faders, live dB meters, mute, audio monitor without recording** |
| Editor | Cloud / heavy | **Local: lossless quick trim + advanced timeline editor (text, stickers, effects, transitions)** |
| Sharing | Locked to vendor cloud link | **Your Drive (public link + clipboard) + library restore on any PC, YouTube, Discord, TikTok** |
| Open source | Usually not | **Yes, GPL-3.0-only** |
| Languages | Usually EN only | **ES + EN from day one (docs in English)** |

## Features

- **Replay buffer with global hotkey** — `F9` saves the last N seconds (default 30s); the embedded, isolated OBS flushes its RAM buffer to disk (no re-encode).
- **Registered games + autopilot** — register a window once; MoonClip starts the buffer when the game appears and stops it when it closes (file managers and browser tabs can't fake a game).
- **Mix-first 3-track audio** — Track 1 = game+mic mix (plays everywhere), Track 2 = game only, Track 3 = mic only. Gains/mutes 0–100 applied live without restarting the buffer.
- **OBS-style mixer + meters** — horizontal 0–100 faders with green/amber/red dB meters and peak hold, mute per track, plus an **audio monitor** that shows device levels without recording (see [`docs/11_AUDIO.md`](./docs/11_AUDIO.md)).
- **Medal-style UI** — icon rail + contextual rail + header, neutral layered blacks with logo red/blue accents, hover previews in the gallery, bilingual ES/EN, low-power mode, pausable starfield. Details: [`docs/07_UI_MOONCLIP.md`](./docs/07_UI_MOONCLIP.md).
- **Advanced editor** — lazy-loaded timeline editor: trim/split/duplicate, text with animations, stickers/GIF search, effects (speed, freeze, zoom, filters, chroma), multi-clip transitions, 3-stem mixing, export with progress/cancel and a libx264 fallback.
- **Gallery** — thumbnails, real durations, favorites, per-game folders, ghost-clip reconcile (never deletes files), LRU quota, loop playback opt-in.
- **Sharing** — Google Drive (resumable upload, public link, cloud-only clips on demand, **library restore on any computer**), YouTube `videos.insert` (private until the API audit passes), Discord ("connect your account", no bot, optional 720p compression for large clips) and TikTok Direct Post via a stateless Cloudflare Worker.
- **Safe persistence** — SQLite with relative paths only; OAuth tokens and webhooks in the OS keyring; `social.json` outside the repo.
- **Zero mixing with your OBS** — MoonClip runs its own generated profile/scene; your `%APPDATA%\obs-studio` / `~/.config/obs-studio` is never read or written.

## Security & secrets — no secrets in git

**The repository is public. Committing secrets is forbidden.** That means:
`social.json`, OAuth client secrets, refresh/access tokens, Discord webhook
URLs, TikTok client secrets, Cloudflare tokens and private keys **never** go
into git (issues, PRs, screenshots included).

- Real credentials live only in: the **OS keyring** (tokens, Discord webhook),
  the gitignored **`social.json`** (`0600`, app-data dir) and **Cloudflare
  Worker secrets** (TikTok client secret).
- Google Desktop client ids and TikTok client keys are public identifiers;
  their secrets are not.
- `scripts/check-secrets.sh` scans the tree and the staged diff and must print
  `clean` before pushing. If a secret ever lands in git: **rotate it first**,
  then rewrite history.

Full policy: [`docs/05_STORAGE_SECURITY.md`](./docs/05_STORAGE_SECURITY.md) (§Secrets policy).

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
- Linux screen capture: the embedded OBS asks the XDG portal for the screen the first time ("Share screen" + Remember). No caps, no `pkexec` prompts.
- Windows: 10 version 1903 (build 18362)+ or 11. MSVC toolchain.
- Sidecars are fetched once into `src-tauri/binaries/`:
  ```bash
  # Linux
  bash build-aux/linux/build-obs.sh       # compile the pinned engine once (patches applied)
  bash build-aux/linux/fetch-ffmpeg.sh    # pinned static ffmpeg sidecar
  ```
  ```powershell
  # Windows
  pwsh build-aux/windows/fetch-obs.ps1      # OBS 32.2.2 (pinned, sha256)
  pwsh build-aux/windows/fetch-ffmpeg.ps1   # editor/probe FFmpeg (pinned)
  ```

### Provider credentials (optional features)

Social integrations read a gitignored `social.json` from the app-data dir
(Linux `~/.local/share/dev.souriscg.moonclip/social.json`, Windows
`%APPDATA%\dev.souriscg.moonclip\social.json`). Copy
[`social.example.json`](./social.example.json) and fill your own client ids.
Discord needs no entry (public client id embedded). **Never commit this file**;
see the security section above and
[`docs/09_WINDOWS_HANDOFF.md`](./docs/09_WINDOWS_HANDOFF.md).

### Develop

```bash
pnpm install
pnpm tauri:dev
```

### Build

```bash
pnpm build                 # frontend (tsc + vite)
pnpm test                  # frontend tests (vitest)
cargo test                 # backend tests (in src-tauri/)
pnpm tauri:build:linux     # RPM with sidecars under /usr/lib/MoonClip/binaries/
pnpm tauri:build:windows   # NSIS + MSI
bash scripts/check-secrets.sh
```

Linux local install (fully embedded: OBS engine + static ffmpeg in the same package):

```bash
pnpm app:install               # replace install + taskbar association
pnpm desktop:install           # dev Wayland app-id association
```

> Windows SmartScreen (early builds): the app is unsigned OSS. Click `More info → Run anyway`. Signing via Store/MSIX comes after Phase 7.

## Usage

- `F9` (default, changeable in Settings → Clip hotkey) — save a clip (global, works over fullscreen games).
- Tray icon — buffer status (active/idle), show/hide, quit. The main window minimizes to tray while gaming.
- Settings — buffer length; quality presets (New Moon 720p → Full Moon 2160p) + custom encoder/video; monitor; **audio mixer** (0–100 faders, meters, mute, audio monitor); accounts (Drive/YouTube/Discord/TikTok); base folder; locale ES/EN; isolated engine panel (status, logs, repair).
- Clips live in your OS videos folder under `MoonClip` (Linux) or `%LOCALAPPDATA%\MoonClip\Clips` (Windows). The DB stores only relative names; move the folder freely.

## Privacy: local-first, zero-account

- No central server, no telemetry, no MoonClip account.
- Clips + SQLite stay on disk. Tokens stay in the OS keyring (Secret Service/KWallet, Credential Manager).
- Uploads go client → service directly (Drive API, YouTube API, Discord webhooks,
  TikTok API). The only server component is the stateless Cloudflare Worker that
  adds the TikTok client secret during OAuth; it stores nothing.
- You revoke access where you created it (Google account, Discord, TikTok).

## Tech stack (brief)

> Gamers can skip this. Contributors: start at [`SPEC.md`](./SPEC.md) + [`docs/01_ARCHITECTURE.md`](./docs/01_ARCHITECTURE.md).

- **App:** Tauri v2 + React 19 + TypeScript + Vite + Tailwind v3 + `react-i18next` + Zustand + `dnd-timeline` + `react-moveable` + Lucide
- **Backend:** Rust (tokio, serde, rusqlite, keyring, rodio, uuid, dirs, image, obws, reqwest/tiny_http/sha2)
- **Capture:** embedded, isolated OBS Studio (Windows: WGC/DXGI + WASAPI; Linux: PipeWire portal + PulseAudio), controlled in-process over a private obs-websocket (`obws`). Display sources only — no game hooks.
- **Sidecars:** `moonclip-engine` (OBS) + static `ffmpeg` in `src-tauri/binaries/` (pinned, sha256-verified)
- **Fonts:** Inter + JetBrains Mono (OFL, bundled locally) and DejaVu Sans for editor text overlays
- **Rules:** hardware-encode first, lossless-cut by default, lazy editor, relative paths, zero `cfg(target_os)` outside `os/`, IPC keys always camelCase, no secrets in git

```
moonclip/
├── docs/                # EN technical spec (01-11 + ROADMAP + PROGRESS + THIRD_PARTY)
├── SPEC.md              # Index + acceptance map
├── workers/moonclip-oauth/  # Cloudflare Worker (TikTok token broker)
├── src-tauri/src/os/    # ALL platform code (linux/ + windows/ behind traits)
├── src-tauri/src/       # social/ storage/ editor/ commands.rs state.rs
├── src/components/      # shell/ ui/ gallery/ settings/ starfield/
├── src/editor/          # lazy advanced editor chunk
└── scripts/             # audit-deps.sh, check-secrets.sh
```

## License

Copyright (C) 2026 SourisCG

This program is free software: you can redistribute it and/or modify it under the terms of the **GNU General Public License version 3 only**, as published by the Free Software Foundation. See [LICENSE](./LICENSE).
