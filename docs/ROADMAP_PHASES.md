# ROADMAP — Phased Execution (Spec-Driven)

**Canonical phase numbers 0–8, one execution order.** Docs are numbered by
topic (`01_*`–`10_*`) and do **not** match phase numbers: each phase lists
its technical doc. Sub-tracks (`3-win`, `1-fixes`, …) belong to their phase.

Execute strictly in order. Do not start phase N+1 until phase N acceptance passes.

| Phase | Scope | Doc | Status |
|---|---|---|---|
| 0 | Architecture, stack rules, IPC contract, docs skeleton | `01_ARCHITECTURE.md` | ✅ done |
| 1 | Scaffold, tray, F9, glass UI, starfield, i18n; Medal-style gallery | `07_UI_MOONCLIP.md` | ✅ done |
| 2 | Storage & security: SQLite relative paths, keyring, LRU quota, reconcile, per-game folders | `05_STORAGE_SECURITY.md` | ✅ done |
| 3 | Capture engine: embedded isolated OBS, 3-track audio, quality ladder | `02_CAPTURE_ENGINE.md` | ✅ done (Linux) |
| 3-win | Windows engine track (WGC, WASAPI, AMF/QSV/x264) | `09_WINDOWS_HANDOFF.md` | 🚧 code done, in-game pass pending |
| 4 | Game detection + launchers + custom apps | `03_GAME_DETECTION.md` | ✅ done |
| 5 | Editor: quick trim (E1) + Medal-style advanced editor (E2–E6) | `04_EDITOR_PIPELINE.md` | ✅ done |
| 6 | Sharing: Google Drive first, other socials later | `06_SOCIAL_INTEGRATIONS.md` | 🚧 Drive done; Discord/Twitter/YouTube/TikTok pending |
| 7 | CI/CD packaging and signing | `08_CI_CD_DISTRIBUTION.md` | ⬜ pending |
| 8 | Distribution & dependencies: decode matrix, dependency audit, any-PC fallbacks | `10_DEPENDENCIES.md` | 🚧 in progress |

## Phase 0 — Architecture

- Rules, stack, tree and IPC contract fixed before code (`01_ARCHITECTURE.md`).
- **Accept:** `pnpm build` + `cargo check` green on the bare scaffold.

## Phase 1 — Scaffold, tray, hotkey, UI base

- Tauri v2 + tray (minimize-to-tray), `global-shortcut` F9 → capture event.
- Tailwind MoonClip theme, `MoonClipStarfield.tsx` canvas (pauses off-screen), glass layout, i18n `en`/`es`.
- **Accept:** F9 fires in any app; minimized <40 MB, 0% CPU; starfield pauses off-screen.
- Later UI work (gallery grid, one clip panel, rename, low-power mode) lands here too.

## Phase 2 — Safe persistence & storage security

- `rusqlite` migrations: `clips`, `custom_apps`, `settings`, `drive_folders` (relative paths only). `keyring` vault for OAuth tokens and the portal password.
- IPC: `list_clips`, `toggle_favorite`, `register_app`, `get_settings/set_settings`, `resolve_clip_src`.
- Hardening: WAL, `0700/0600` app data, CSP, asset protocol removed, LRU quota that never prunes favorites, boot reconcile (never deletes files), per-game folders.
- **Accept:** CRUD works; `base_dir + file_name` resolves; no absolute path in DB; secrets only in the OS vault.

## Phase 3 — Capture engine (embedded OBS replay + 3-track audio)

- `CaptureEngine` trait + shared `os/shared/engine.rs`: isolated portable OBS copy, generated `MoonClip` profile/scene (display/window sources only, never `game_capture`), private obs-websocket, `rodio` ding, tray status.
- Windows: `monitor_capture` (DXGI duplication) + WASAPI; Linux: PipeWire portal + PulseAudio.
- **Accept:** F9 → `.mp4` with 3 audio tracks (Mix first); indexed in DB with thumbnail; the user's own OBS config is never touched.

## Phase 3-win — Windows engine track

- Toolchain, stubs and acceptance in `09_WINDOWS_HANDOFF.md`.
- **Accept:** in-game F9 pass on Windows (BO7/CS2/Vanguard), coexistence with a system OBS install.

## Phase 4 — Game detection + launchers

- GPU FD scan, Wine cmdline parser + blacklist, `SteamAppId` + `.acf`, Minecraft/Prism, Heroic/Epic manifests, Battle.net child, Xbox title, `get_running_applications` + matcher.
- Window matching rejects known non-game apps and title suffixes (`" - Brave"`, `"— Dolphin"`, …); custom apps override duration.
- **Accept:** native + Wine/Proton + Minecraft report correct titles; a browser window never fakes a game.

## Phase 5 — Editor (quick trim + Medal-style advanced editor)

- **Quick trim (E1):** one ffmpeg run (lossless `-c copy`, precise libx264), new clip indexed with thumbnail, faststart copy for instant playback.
- **Advanced editor (E2–E6):** lazy maximized Medal-style view, timeline, preview overlays, text/stickers/effects (libass text with rotation), freeze frames, multi-clip + transitions, 3 audio tracks visible and single-track export; project JSON with autosave; export through the ffmpeg sidecar with progress/cancel; libx264 fallback.
- **Accept (E1):** lossless <1 s; precise exact; 3 audio tracks preserved. **Accept (E6):** RAM back to baseline on close; export E2E 1 video + 1 audio.

## Phase 6 — Sharing (Drive + social)

- Drive PKCE loopback + resumable chunks + progress + public link + clipboard; uploads persisted per clip (never duplicated); cloud-only clips with on-demand download; Discord webhook; Twitter intent+clipboard; YouTube (+`#Shorts`); TikTok draft/fallback.
- **Accept:** Drive URL public + copied + notified; Discord/Twitter/YouTube flows work.

## Phase 7 — CI/CD packaging

- Narrow `tauri.conf.json` targets, `release.yml` matrix, SmartScreen README note, versioned Release assets.
- **Accept:** `v*` tag produces `.exe/.msi/.AppImage/.rpm/.deb` downloadables.

## Phase 8 — Distribution & dependencies (any PC)

- `10_DEPENDENCIES.md`: shipped components, generated system-lib table, optional hardware-decode matrix per GPU/vendor with per-distro package names, install commands (doc only — the app shows the package name, never runs distro commands).
- `scripts/audit-deps.sh` regenerates the table (`ldd` + local package DB).
- Any-PC fallbacks: DMA-BUF renderer self-healing + low-power UI mode, software decoders always bundled (no NVENC/proxy requirement), Wayland-only on Linux.
- **Accept:** app runs with and without a GPU decode driver (correct Settings notice), dependency table regenerates, bundles declare their deps.

---

## Agent prompt pattern (per phase)

> "Follow `SPEC.md` + `docs/XX_*.md`. Implement ONLY Phase N. Do not write later-phase logic. Verify acceptance checklist before finishing."
