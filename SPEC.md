# MoonClip — SPEC (Index)

> Open-source, lightweight, zero-cloud Medal.tv alternative for Linux + Windows. Bilingual app (ES/EN). Docs in English.

## Status: Phase 3 done on Linux; Windows V2 rewrite complete (user in-game pass pending)

- Working (Linux, user-verified): tray + F9 replay buffer (GSR embedded),
  3-track mix-first audio, live gains, Medal CBR ladder + NVENC HQ, 30/60fps,
  monitor select, lanczos-on-save, gallery with thumbs + real durations,
  settings (SQLite relative paths) + OS keyring vault, frameless MoonClip UI.
- Windows V2 (2026-09-17, `docs/09_WINDOWS_HANDOFF.md`): same IPC surface with
  a rewritten engine under `os/windows/` (WGC `gfxcapture` + DXGI `ddagrab`
  fallback → NVENC/AMF/QSV/x264 → TS ring → copy-only save), WASAPI audio
  (`wasapi` crate, QPC timestamps, keep-alive, watchdog), §10 container
  (320/320/192 + titles, faststart, mp4/mkv), Medal ladder incl. 480p/2160p +
  CQP export, QualityTable + SetupWizard UI (moon presets, custom mode).
  Gates: clippy `-D warnings`, 58 unit tests, `pnpm build`, zero-`cfg` grep.
  Measured: saves 0.23–0.29 s, A/V ±8 ms, 120 s save 3.44 s on SATA.
  Pending: BO7 10-min stress (1080p60/1440p60), 30-min drift, Kdenlive
  multitrack, legacy FSE, Linux CI regression.
- HDR is out of V2 (capture treated as normal SDR video); PID-isolated
  loopback is left to the Linux owner (logged in PROGRESS).
- Stack: Tauri v2 + React 19 + TS + Vite + Tailwind v3 + `react-i18next` +
  Wavesurfer + Lucide. Rust: tokio, serde, rusqlite, keyring, rodio, uuid,
  dirs, nix (Linux), image, wasapi (Windows). Package manager: **pnpm**.

## Doc map

- `docs/01_ARCHITECTURE.md` — rules, stack, tree, IPC contract.
- `docs/02_CAPTURE_ENGINE.md` — GSR Linux (`SIGUSR1`, `-r`, triple `-a`), ladder + HQ recipe, lanczos-on-save, monitor select.
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
