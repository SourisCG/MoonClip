# 09 — Windows Handoff

Read this first, then `SPEC.md`, then `01_ARCHITECTURE.md` (rule 6 is law),
then `02_CAPTURE_ENGINE.md` (engine) and `11_AUDIO.md` (mixer/meters).

> **NO SECRETS IN GIT.** The repository is public. `social.json`, client
> secrets, OAuth tokens, Discord webhook URLs, TikTok/Cloudflare credentials —
> none of them go into git, issues, PRs or screenshots. Real values live only
> in the Windows Credential Manager, the gitignored `social.json` and the
> Cloudflare Worker secrets. See `05_STORAGE_SECURITY.md` §Secrets policy and
> run `bash scripts/check-secrets.sh` (must print `clean`) before pushing.

> **Status (2026-09-28):** the Windows engine, social integrations, audio
> subsystem and Medal UI v2 are all implemented in code and unit/mock-tested on
> Linux. The work left on Windows is: build it, create `social.json`, and run
> the live checklist in §8. **What's missing is listed in §12.**

## 1. How to work here

- Toolchain: MSVC (VS Build Tools) + Rust stable + Node 20 + `pnpm install`.
  Dev loop: `pnpm tauri:dev`. Verify:
  `pnpm build`, `pnpm test`, `cargo check --target x86_64-pc-windows-msvc`,
  `cargo test`, `cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings`,
  and `bash scripts/check-secrets.sh` (Git Bash) or the equivalent check by eye.
- Sidecars first: `pwsh build-aux/windows/fetch-ffmpeg.ps1` (editor) and
  `pwsh build-aux/windows/fetch-obs.ps1` (engine 32.2.2, pinned; stages
  `engine/bin/64bit/moonclip-engine.exe`).
- Frontend tests are vitest (`pnpm test`); do not add another runner.
- Minimum supported OS: **Windows 10 version 1903 (build 18362) or later**
  (WGC floor; the installer targets 1903+).
- Package manager is **pnpm** (never npm). Commits: small, conventional
  (`feat/fix/docs`), push to `origin/main` when green.
- Zero-`cfg` rule: NO `cfg(target_os)` and NO OS APIs outside `src-tauri/src/os/`.
  The grep in `01_ARCHITECTURE.md` rule 6 must print nothing.
- IPC rule: `#[tauri::command]` auto-converts Rust `snake_case` params to
  **camelCase** wire keys (`clip_id` → `clipId`). Frontend always sends
  camelCase. Struct payloads stay snake_case.
- No human text crosses IPC: the backend returns ids/codes, the frontend
  translates via `src/locales/{en,es}.json`. Docs are English.
- License is **GPL-3.0-only** (`LICENSE`, `docs/THIRD_PARTY.md`).

## 2. Engine architecture (V3, frozen)

```
[settings] -> os/shared/engine.rs::write_obs_config(root, profile)
               root/obs-studio/basic/profiles/MoonClip/basic.ini
               root/obs-studio/basic/profiles/MoonClip/recordEncoder.json
               root/obs-studio/basic/scenes/MoonClip.json
               root/obs-studio/global.ini
               root/obs-studio/plugin_config/obs-websocket/config.json
            -> stage a writable portable OBS copy at %LOCALAPPDATA%\MoonClip\obs
               (copy + portable_mode.txt + build marker; once per build)
               staged exe renamed to moonclip-obs.exe (unmistakable in TM)
                           --collection MoonClip --multi
                           --disable-shutdown-update-check --disable-updater
                           --only-bundled-plugins
            -> conceal_window (user.ini without tray + Win32 watcher)
            -> config guard (root/obs-studio must appear, else abort)
            -> connect obs-websocket + subscribe meters + `replay start`
F9         -> obs-websocket replay save -> poll `last-replay` -> DB index
```

- `root` = `%LOCALAPPDATA%\MoonClip\obs` — NEVER `%APPDATA%\obs-studio`.
- Websocket: `127.0.0.1:<engine_ws_port>` (default 4456) with a password
  generated **per start** (never persisted); MoonClip's in-process `obws`
  client is the only client.
- Encoder mapping (`os/windows/engine.rs`): NVENC
  `obs_nvenc_{h264,hevc,av1}_tex`, AMF `h264_texture_amf` / `h265_texture_amf`
  / `av1_texture_amf`, QSV `obs_qsv11_v2` / `obs_qsv11_hevc` / `obs_qsv11_av1`,
  CPU `obs_x264`.
- Capture source: `monitor_capture` (DXGI Desktop Duplication, `method: 1`,
  monitor id) or opt-in `window_capture` (WGC). **Never** `game_capture`
  (anti-cheat).
- Audio: `wasapi_output_capture` (loopback) + `wasapi_input_capture`; ids come
  from the WASAPI enumeration (`os/windows/devices.rs`);
  `default_output`/`default_input` map to OBS's `default`.
- Concealment: Windows hides the engine through `user.ini`
  (`SysTrayEnabled=false`) plus a Win32 watcher matching the exact PID + image;
  dialogs are left intact. This is the platform counterpart of Linux's KWin
  script (`os/linux/engine.rs`).

## 3. Module map (`src-tauri/src/os/`)

| File | Responsibility |
|---|---|
| `api.rs` | `CaptureConfig` + `CaptureEngine` trait (frozen surface) |
| `shared/engine.rs` | shared engine: profile/scene writers, `ObsEngine`, config guard, log ring, gain helpers |
| `shared/obsws.rs` | obs-websocket v5 client (`obws`): replay/scene/source/video/input ops + meter subscription |
| `shared/audio_meters.rs` | `InputVolumeMeters` pump, level/peak store with decay (`11_AUDIO.md`) |
| `shared/winlist.rs` | OS-free window matching for the game autopilot (`03_GAME_DETECTION.md`) |
| `shared/encoder_options.rs` | pinned encoder catalog + per-family option specs |
| `windows/engine.rs` | portable-copy staging, launch args, encoder ids, DXGI source, concealment, orphan sweep |
| `windows/binary.rs` | resolve the bundled engine exe / ffmpeg (env override) |
| `windows/video.rs` | DXGI vendor, GDI monitor list, ffmpeg-probed codec offer |
| `windows/devices.rs` | WASAPI endpoint enumeration + magic-default mapping |
| `windows/winlist.rs` | `EnumWindows` list (title + class + exe) |
| `linux/*` | Linux mirror (portal, PulseAudio, KWin concealment, /proc sweep) |
| `mod.rs` | per-OS selection + `new_engine()`, `obs_config_root`, `memory_free_mb` |

## 4. Audio (3 tracks, Mix first, live mixer)

- Tracks: `RecTracks=7`, Track 1 `Master Mix [Game+Voice]` 320k, Track 2
  `Game/Desktop` 320k, Track 3 `Microphone` 192k; game source mixers `1|2`,
  mic `1|4`. `audio_single_track` collapses to 1 track.
- Gains are **0–100** (100 = unity/maximum), written into the scene and applied
  live through `SetInputVolume` while the buffer runs; a failed live apply is
  logged/surfaced and never restarts the buffer.
- Mutes apply live through `SetInputMute`.
- **Meters**: the engine subscribes to `InputVolumeMeters`; `audio_peaks`
  returns `{game, game_peak, mic, mic_peak}` and the UI draws the OBS-style dB
  scale with zones and peak hold.
- **Audio monitor**: `start_audio_monitor`/`stop_audio_monitor` run an
  audio-only collection (no capture input, no replay buffer) so levels work
  without recording. Full details: `11_AUDIO.md`.
- Devices: `list_audio_devices` enumerates via WASAPI.

## 5. Quality ladder and custom mode

- Shared `video_quality.rs`: Medal CBR ladder + `recommended_kbps` +
  `ring_mb`.
- Settings keys: `video_codec` (h264/hevc/av1; legacy `x264` → h264+cpu),
  `video_encoder` (`gpu`|`cpu`), `gpu_index`, `out_height` (0 = source),
  `fps` (24/30/60/120/144), `buffer_seconds`, `video_mode`
  (`ladder`|`custom`), `custom_bitrate_kbps` (3–100 Mbps), `custom_fps`,
  `custom_encoder_json`, `custom_video_json` (migration 008), `container`,
  `monitor`, `engine_ws_port`, gains/mutes, `audio_single_track`,
  `discord_max_mb`.
- UI: `VideoSection.tsx` (Moon preset cards + Custom panel per encoder) +
  shared `NumberField.tsx` + `ObsEngineSection.tsx` (status, activity ring,
  Repair).
- `RESTART_KEYS` in `commands.rs` restarts the buffer once per settings change
  (with revert-on-failure); gains/mutes apply live first.

## 6. Social integrations (platform-neutral)

Implemented in `social/` exactly as on Linux (no per-OS code):

- **Google Drive**: connect, resumable upload, public link, cloud-only clips,
  **library restore** (`drive_sync_library`).
- **YouTube**: `youtube_share_clip` (private until the API audit passes).
- **Discord**: `connect_discord` (OAuth `webhook.incoming`, fixed loopback
  `127.0.0.1:38471`), `discord_share_clip` (optional 720p compression).
- **TikTok**: `connect_tiktok`, `tiktok_creator_info`, `tiktok_share_clip`
  through the Cloudflare Worker.

**Credentials (never in git):** create
`%APPDATA%\dev.souriscg.moonclip\social.json` (file `0600`-equivalent,
gitignored) from `social.example.json`. The file may contain the Google
Desktop client id/secret and the TikTok client key + Worker URL; the TikTok
client secret stays only in the Worker (`wrangler secret put`). Env overrides:
`MOONCLIP_GOOGLE_DRIVE_CLIENT_ID/_SECRET`, `MOONCLIP_DISCORD_CLIENT_ID`,
`MOONCLIP_TIKTOK_WORKER_URL`. **Never commit this file** — see the top warning
and `05_STORAGE_SECURITY.md`.

## 7. Commands (IPC surface)

- Capture/mixer: `start_buffer`, `stop_buffer`, `start_audio_monitor`,
  `stop_audio_monitor`, `engine_status`, `save_clip_now`, `video_options`,
  `test_hardware`, `obs_info`, `repair_obs_config`, `list_audio_devices`,
  `audio_levels`, `audio_peaks`, `set_track_gain`, `set_track_mute`.
- Social: `social_status`, `connect_google_drive`, `connect_google_youtube`,
  `youtube_share_clip`, `connect_discord`, `discord_share_clip`,
  `connect_tiktok`, `tiktok_creator_info`, `tiktok_share_clip`,
  `drive_upload_clip`, `drive_browse`, `drive_download`, `drive_sync_library`.
- Library/editor/settings: as in `01_ARCHITECTURE.md` §5.

## 8. Live checklist on Windows (the whole app, end to end)

Build + boot:

1. `pnpm install`, sidecars, `pnpm tauri:dev` → the window opens, no console
   errors, `pnpm build`/`cargo test`/`clippy` green.
2. Register a game (Games → Register game) with the OBS source-properties
   dialog; the window is concealed (no tray icon, no window flash).
3. Start the buffer → **no OBS window ever visible**; F9 saves a clip with
   **3 audio tracks** (Mix first); ding plays.

Audio (docs/11):

4. Settings → Audio → **Monitor**: meters move with voice/system audio without
   recording; the monitor stops when leaving Settings.
5. Move both faders 0–100 while the buffer runs → volume changes live, no
   buffer restart, dB readout and zones look like OBS; mute works live; the
   engine log shows the apply lines.

Social (docs/06):

6. Connect Google Drive (same account as Linux) → **library restore** fills the
   gallery with cloud clips + thumbnails; open one (downloads on demand).
7. Upload a new clip to Drive, make it public, copy the link; delete it and
   confirm the remote goes to the Drive trash.
8. YouTube: connect and upload (video stays private until the audit).
9. Discord: connect (server + channel with *Manage Webhooks*) and send a clip;
   force the compression path with a clip over `discord_max_mb`.
10. TikTok: connect with the sandbox target user (account set to **private**)
    and publish `SELF_ONLY`; the privacy dropdown starts empty by design.

UI v2 (docs/07):

11. Icon rail + context rail (collapse button remembers its state), header
    search/sort, hover video previews in the gallery.
12. Both editors show the position indicator: quick-trim timeline and advanced
    timeline draw a red playhead with a handle; the advanced scrub bar under
    the video has a round thumb.
13. Windows-only quick pass: no washed/transparent surfaces, native controls
    look identical to Linux (custom Select/checkbox/fader/scrollbars).

## 9. Test rig

- Buffer/save smoke: `pnpm tauri:dev`, Start, move the mouse, F9,
  `%LOCALAPPDATA%\MoonClip\Clips`.
- Tracks: `ffmpeg -i <clip>` shows 3 streams + titles; VLC → Audio → Track.
- Isolation: open the user's OBS, then MoonClip Start; both run, and
  `%APPDATA%\obs-studio` is untouched (compare mtimes).
- Anti-cheat grep: `git grep -n game_capture` must only show guard constants,
  comments and tests (never a generated ini/json string).
- Hardware test: Settings wizard → "Test my PC" → result summary.
- Secrets: `bash scripts/check-secrets.sh` before every push.

## 10. Packaging

- `src-tauri/tauri.windows.conf.json` bundles the engine directory and the
  FFmpeg sidecar as resources; `pnpm tauri:build:windows` produces NSIS + MSI.
- `build-aux/windows/fetch-obs.ps1` pins OBS `32.2.2` (sha256 verified) and
  extracts the portable layout (the script never executes OBS). Control is
  in-process over obs-websocket (`obws`); no `obs-cmd` binary ships.
- SmartScreen applies to unsigned builds (see `08_CI_CD_DISTRIBUTION.md`).
- **Do not package `social.json`** — it is per-user config created at first
  run (see `05_STORAGE_SECURITY.md`).

## 11. Platform differences (Linux ↔ Windows)

| Area | Linux | Windows |
|---|---|---|
| Capture | PipeWire portal (`pipewire-desktop-capture-source`) | `monitor_capture` (DXGI) / `window_capture` (WGC) |
| Audio backend | PulseAudio sources | WASAPI loopback + input |
| Engine isolation | `XDG_CONFIG_HOME` redirect, no copy | staged writable portable copy in `%LOCALAPPDATA%\MoonClip\obs` |
| Concealment | KWin DBus script (Wayland/X11) | `user.ini` + Win32 PID/image watcher |
| Orphan sweep | `/proc` scan | Win32 job/child tracking |
| Keyring | Secret Service/KWallet | Credential Manager |
| Default clips dir | OS videos folder + `MoonClip` | `%LOCALAPPDATA%\MoonClip\Clips` |
| `social.json` path | `~/.local/share/dev.souriscg.moonclip/` | `%APPDATA%\dev.souriscg.moonclip\` |
| Local install | `pnpm app:install` (RPM) | **missing** — see §12 |

## 12. What's missing on Windows (TODO for the Windows agent)

1. **Live passes** of everything in §8 (the code is unit/mock-tested but the
   in-game F9 pass and the social clicks are owner-verified work).
2. **Local install/dev script**: there is no Windows equivalent of
   `build-aux/linux/install-local.sh` / `install-dev-desktop.sh`. Add
   `build-aux/windows/install-local.ps1` (install the built NSIS/MSI or a
   portable layout, register the app id, keep the engine/ffmpeg resources
   together) and document it here.
3. **Dependency audit**: run `dumpbin /dependents` (or the Dependencies tool)
   over `moonclip.exe`, `moonclip-engine.exe` and the ffmpeg sidecar, and append
   the table to `docs/10_DEPENDENCIES.md` §5.
4. **Concealment verification**: confirm the Win32 watcher never lets the OBS
   window flash (Task Manager shows `moonclip-obs.exe` as a child, no stray
   "OBS Studio" entry) — the Linux KWin script was reverted to its proven form
   (`cd3dbda`); do not add extra signal connects there.
5. **In-game pass** (Phase 3-win pending): BO7/CS2/Vanguard, system-OBS
   coexistence, Repair, hardware-test wizard, 30-min drift if touched.
6. **CI**: workflows do not exist yet (Phase 7); when added, signing keys live
   in repository secrets and no secret is ever written to disk in the repo.

## 13. Checklist before pushing

- [ ] No `sh`/`xdg-open`/`/proc`/`getcap`/`pkexec` on Windows paths.
- [ ] Clips live under `%LOCALAPPDATA%\MoonClip\Clips` by default (AV-safe).
- [ ] No hardcoded UI text (backend returns ids; locales cover EN+ES).
- [ ] No absolute paths in DB; relative names only.
- [ ] `game_capture` never generated; portable-copy + config guard intact.
- [ ] Behaviors above work without reshaping `commands.rs` IPC.
- [ ] **No secrets staged**: `social.json`, client secrets, tokens, webhook
      URLs. `scripts/check-secrets.sh` prints `clean`.
