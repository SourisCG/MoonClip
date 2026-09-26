# MoonClip — SPEC (Index)

> Open-source, lightweight, zero-cloud Medal.tv alternative for Linux + Windows. Bilingual app (ES/EN). Docs in English.

## Status: V4 — game detection, window capture and auto-buffer (Linux done; Windows pass pending)

- Capture engine (V3.x): BOTH OSes run one shared, embedded and isolated
  engine driven in-process over obs-websocket v5 (`obws`) — `obs-cmd` was
  removed in V3.5 (see `docs/02_CAPTURE_ENGINE.md`). GSR and the ffmpeg
  `gfxcapture` engine are gone. The user's own OBS is never touched
  (`XDG_CONFIG_HOME` on Linux, portable copy on Windows) and `game_capture`
  is forbidden (anti-cheat): display/window sources only.
- Identity (V3.6): the engine is a patched build (`build-aux/patches/`,
  `build-aux/linux/build-obs.sh`); binaries/threads/audio/portal/disk/DB/UI
  never expose the upstream name. Verification scripts live in
  `build-aux/linux/tests/`.
- V4 (current, Linux done): game detection (read-only `/proc` + manifests;
  Windows ToolHelp/registry code ready for the owner's pass), **window
  capture** (X11/XWayland via `xcomposite_input`; Wayland native via portal
  window session with a per-game restore token; one window choice per game,
  monitor remains a manual option), Medal-style auto-buffer, per-game clip
  duration, icons (Steam/Prism/.desktop). Pending: real-game spike (KDE
  window-token restore), click-to-pick-window crosshair, Windows exe icons.
- HDR is out of scope (capture treated as normal SDR video).
- Stack: Tauri v2 + React 19 + TS + Vite + Tailwind v3 + `react-i18next` +
  Wavesurfer + Lucide. Rust: tokio, serde, rusqlite, keyring, rodio, uuid,
  dirs, nix (Linux), image, wasapi (Windows). Package manager: **pnpm**.

## Doc map

- `docs/01_ARCHITECTURE.md` — rules, stack, tree, IPC contract.
- `docs/02_CAPTURE_ENGINE.md` — embedded engine: isolation, identity/stealth, anti-cheat sources, profile/scene writers, obs-websocket contract, 3-track audio, quality ladder, hardware test.
- `docs/03_GAME_DETECTION.md` — detection sources and matcher (as-built target for V4).
- `docs/10_GAME_DETECTION_PLAN.md` — live V4 plan: window capture, per-game tokens, auto-buffer, sub-phases + acceptance. (Phase 4)
- `docs/04_EDITOR_PIPELINE.md` — lazy `ClipEditor`, Wavesurfer Regions, FFmpeg sidecar (lossless/vertical/remix), keyframe note. (Phase 5)
- `docs/05_STORAGE_SECURITY.md` — SQLite relative paths, `clips`/`custom_apps`/`settings`, ghost-clip reconcile, LRU prune, `keyring`.
- `docs/06_SOCIAL_INTEGRATIONS.md` — Drive PKCE + resumable + public link, Discord/Twitter/YouTube/TikTok, IG/FB deferred. (Phase 6, platform-neutral)
- `docs/07_UI_MOONCLIP.md` — palette, pausable starfield, glass layout, i18n, WebView2/WebKit notes, tray/taskbar icons.
- `docs/08_CI_CD_DISTRIBUTION.md` — `nsis/msi/appimage/deb/rpm` (+`msix` later), signing/MS Store/WinGet/Flathub. (Phase 7)
- `docs/09_WINDOWS_HANDOFF.md` — **start here on Windows**: toolchain, stub inventory + contracts, acceptance, checklist.
- `docs/THIRD_PARTY.md` — engine/ffmpeg pins, identity patch set and ship matrix.
- `docs/ROADMAP_PHASES.md` — phased acceptance checklists.
- `docs/PROGRESS.md` — build log (single source of truth for status).

## How to work (OpenCode / Cursor / Windsurf)

1. Read this file + `09_WINDOWS_HANDOFF.md` (on Windows) + the relevant `docs/0X_*.md` (`10` for V4).
2. Implement **only** the requested scope (one numbered sub-phase at a time); keep `commands.rs` contracts stable.
3. Respect non-negotiables in `01_ARCHITECTURE.md` (relative paths, lossless-first, lazy editor, zero-cloud, zero-`cfg` outside `os/`, no human text over IPC, camelCase wire keys).
4. Verify acceptance checklist with real commands (`pnpm build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, build-aux/linux/tests/*; `ffprobe` for clips).
