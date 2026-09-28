# MoonClip — SPEC (Index)

> Open-source, lightweight, zero-account Medal.tv alternative for Linux + Windows. Bilingual app (ES/EN). Docs in English.

## Status: V3 — embedded isolated OBS engine (Windows + Linux) + Medal UI v2

- Capture engine (V3): BOTH OSes run one shared engine — an embedded, isolated
  OBS Studio driven in-process through obs-websocket (`obws`), see
  `docs/02_CAPTURE_ENGINE.md`. GSR, the ffmpeg `gfxcapture` engine and the
  `obs-cmd` CLI are gone. The user's own OBS is never touched (portable
  config dir + generated `MoonClip` profile/collection + private websocket),
  and `game_capture` is forbidden (anti-cheat): display/window sources only.
- Audio (2026-09-28): live mixer 0–100 with OBS-style dB meters, mute and an
  audio-only monitor mode — `docs/11_AUDIO.md`.
- UI v2 (2026-09-28): Medal-style shell (icon rail + context rail + header),
  neutral layered blacks, logo red/blue accents, custom controls for
  WebKitGTK/WebView2 parity — `docs/07_UI_MOONCLIP.md`.
- Social: Drive (+ library restore), YouTube, Discord (OAuth webhook, no bot)
  and TikTok (Direct Post through a stateless Cloudflare Worker) implemented;
  Twitter/X pending — `docs/06_SOCIAL_INTEGRATIONS.md`.
- Windows verification pending in-game (BO7/CS2/Vanguard), hardware-test
  wizard, system-OBS coexistence, Repair; the social/audio/UI live passes are
  listed in `docs/09_WINDOWS_HANDOFF.md`.
- HDR is out of scope (capture treated as normal SDR video).
- Stack: Tauri v2 + React 19 + TS + Vite + Tailwind v3 + `react-i18next` +
  `dnd-timeline`/`react-moveable` + Zustand + Lucide. Rust: tokio, serde,
  rusqlite, keyring, rodio, uuid, dirs, image, obws, reqwest/tiny_http/sha2,
  nix (Linux), wasapi + windows (Windows). Package manager: **pnpm**.
- **NO SECRETS IN GIT** (hard rule, see `docs/05_STORAGE_SECURITY.md`
  §Secrets policy and `scripts/check-secrets.sh`).

## Doc map

- `docs/01_ARCHITECTURE.md` — rules, stack, real tree, IPC contract. (Phase 0)
- `docs/02_CAPTURE_ENGINE.md` — embedded OBS engine: isolation, anti-cheat sources, profile/scene writers, obs-websocket contract, 3-track audio, quality ladder, hardware test. (Phase 3)
- `docs/03_GAME_DETECTION.md` — window-identity registration (`winlist` + picker), matching guards, autopilot, per-game folders. (Phase 4)
- `docs/04_EDITOR_PIPELINE.md` — lazy advanced editor, dnd-timeline + canvas, FFmpeg sidecar (lossless/vertical/remix), keyframe note. (Phase 5)
- `docs/05_STORAGE_SECURITY.md` — SQLite relative paths, schema 001–015, keyring aliases, `social.json`, secrets policy, ghost-clip reconcile, LRU prune. (Phase 2)
- `docs/06_SOCIAL_INTEGRATIONS.md` — Drive + library restore, YouTube, Discord, TikTok + Worker, Twitter plan. (Phase 6, platform-neutral)
- `docs/07_UI_MOONCLIP.md` — v2 design tokens, Medal shell, component kit, cross-webview rules, starfield/low-power. (Phase 1)
- `docs/08_CI_CD_DISTRIBUTION.md` — `nsis/msi/appimage/deb/rpm` (+`msix` later), signing/MS Store/WinGet/Flathub. (Phase 7)
- `docs/09_WINDOWS_HANDOFF.md` — **start here on Windows**: toolchain, sidecars, `social.json`, end-to-end test checklist, known gaps. (Phase 3-win)
- `docs/10_DEPENDENCIES.md` — shipped components, generated Linux system-lib table, optional hardware-decode matrix, per-distro package names, Windows bundle checklist, audit script. (Phase 8)
- `docs/11_AUDIO.md` — capture audio pipeline, gains/mutes, `InputVolumeMeters`, monitor mode, mixer UI, logs and tests.
- `docs/THIRD_PARTY.md` — licenses + ship matrix (embedded OBS engine; `obws` in-process).
- `docs/ROADMAP_PHASES.md` — phased acceptance checklists.
- `docs/PROGRESS.md` — build log (single source of truth for status).

## How to work (OpenCode / Cursor / Windsurf)

1. Read this file + `09_WINDOWS_HANDOFF.md` (on Windows) + the relevant `docs/0X_*.md`.
2. Implement **only** the requested scope; keep `commands.rs` contracts stable.
3. Respect non-negotiables in `01_ARCHITECTURE.md` (relative paths, lossless-first,
   lazy editor, zero-cloud, zero-`cfg` outside `os/`, no human text over IPC,
   camelCase wire keys).
4. **Never commit secrets** (`social.json`, client secrets, tokens, webhook
   URLs). If one lands in git, rotate it first, then rewrite history.
5. Verify acceptance checklists with real commands (`pnpm build`, `pnpm test`,
   `cargo check`, `cargo test`, `cargo clippy -- -D warnings`, `ffprobe`).
   `scripts/check-secrets.sh` must print `clean`.
