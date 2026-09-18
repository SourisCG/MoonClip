# MoonClip — SPEC (Index)

> Open-source, lightweight, zero-cloud Medal.tv alternative for Linux + Windows. Bilingual app (ES/EN). Docs in English.

## Status: V3 — embedded isolated OBS engine (Windows + Linux)

- Capture engine (V3, 2026-09-18): BOTH OSes now run one shared engine — an
  embedded, isolated OBS Studio driven by the bundled `obs-cmd` (see
  `docs/02_CAPTURE_ENGINE.md`). GSR and the ffmpeg `gfxcapture` engine are
  gone. The user's own OBS is never touched (`--config-dir` + generated
  `MoonClip` profile/collection + private websocket), and `game_capture` is
  forbidden (anti-cheat): display/window sources only.
- Windows verification pending in-game (BO7/CS2/Vanguard), hardware-test
  wizard, system-OBS coexistence, Repair.
- Linux: portal picker once, unpacked-OBS/system-OBS fallback, 3-track clip.
- HDR is out of scope (capture treated as normal SDR video).
- Stack: Tauri v2 + React 19 + TS + Vite + Tailwind v3 + `react-i18next` +
  Wavesurfer + Lucide. Rust: tokio, serde, rusqlite, keyring, rodio, uuid,
  dirs, nix (Linux), image, wasapi (Windows). Package manager: **pnpm**.

## Doc map

- `docs/01_ARCHITECTURE.md` — rules, stack, tree, IPC contract.
- `docs/02_CAPTURE_ENGINE.md` — embedded OBS engine: isolation, anti-cheat sources, profile/scene writers, obs-cmd contract, 3-track audio, quality ladder, hardware test.
- `docs/03_GAME_DETECTION.md` — GPU FD filter, Wine cmdline + blacklist, `SteamAppId` + `.acf`, Minecraft/Prism/Bedrock, Heroic/Epic/Battle.net/Xbox, `custom_apps` + picker + matcher. (Phase 4)
- `docs/04_EDITOR_PIPELINE.md` — lazy `ClipEditor`, Wavesurfer Regions, FFmpeg sidecar (lossless/vertical/remix), keyframe note. (Phase 5)
- `docs/05_STORAGE_SECURITY.md` — SQLite relative paths, `clips`/`custom_apps`/`settings`, ghost-clip reconcile, LRU prune, `keyring`.
- `docs/06_SOCIAL_INTEGRATIONS.md` — Drive PKCE + resumable + public link, Discord/Twitter/YouTube/TikTok, IG/FB deferred. (Phase 6, platform-neutral)
- `docs/07_UI_MOONCLIP.md` — palette, pausable starfield, glass layout, i18n, WebView2/WebKit notes, tray/taskbar icons.
- `docs/08_CI_CD_DISTRIBUTION.md` — `nsis/msi/appimage/deb/rpm` (+`msix` later), signing/MS Store/WinGet/Flathub. (Phase 7)
- `docs/09_WINDOWS_HANDOFF.md` — **start here on Windows**: toolchain, stub inventory + contracts, acceptance, checklist.
- `docs/THIRD_PARTY.md` — GSR pin + ship matrix + scaler-patch schedule.
- `docs/ROADMAP_PHASES.md` — phased acceptance checklists.
- `docs/PROGRESS.md` — build log (single source of truth for status).

## How to work (OpenCode / Cursor / Windsurf)

1. Read this file + `09_WINDOWS_HANDOFF.md` (on Windows) + the relevant `docs/0X_*.md`.
2. Implement **only** the requested scope; keep `commands.rs` contracts stable.
3. Respect non-negotiables in `01_ARCHITECTURE.md` (relative paths, lossless-first, lazy editor, zero-cloud, zero-`cfg` outside `os/`, no human text over IPC, camelCase wire keys).
4. Verify acceptance checklist with real commands (`pnpm build`, `cargo check`, `cargo test`, `ffprobe`).
