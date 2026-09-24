# 09 — Windows Handoff (V3, embedded OBS engine)

Read this first, then `SPEC.md`, then `01_ARCHITECTURE.md` (rule 6 is law),
then `02_CAPTURE_ENGINE.md` (the whole V3 engine). V3 (2026-09-18) replaced
BOTH old engines (GSR on Linux, ffmpeg `gfxcapture` on Windows) with one shared
engine: the embedded, isolated OBS Studio driven in-process over obs-websocket (`obws`).

## 1. How to work here

- Toolchain: MSVC (VS Build Tools) + Rust stable + Node 20 + `pnpm install`.
  Dev loop: `pnpm tauri:dev`. Verify: `pnpm build`,
  `cargo check --target x86_64-pc-windows-msvc`, `cargo test`,
  `cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings`.
- Sidecars first: `pwsh build-aux/fetch-ffmpeg.ps1` (editor) and
  `pwsh build-aux/fetch-obs.ps1` (OBS 32.2.2, pinned).
- Minimum supported OS: **Windows 10 version 1903 (build 18362) or later**
  (WGC floor; the installer targets 1903+).
- Package manager is **pnpm** (never npm). Commits: small, conventional
  (`feat/fix/docs`), push to `origin/main` when green.
- Zero-`cfg` rule: NO `cfg(target_os)` and NO OS APIs outside `src-tauri/src/os/`.
  The grep in `01_ARCHITECTURE.md` rule 6 must print nothing (os/mod.rs's own
  platform selection is the only allowed place).
- IPC rule: `#[tauri::command]` auto-converts Rust `snake_case` params to
  **camelCase** wire keys (`clip_id` → `clipId`). Frontend always sends
  camelCase. Struct payloads stay snake_case.
- No human text crosses IPC: backend returns ids/codes, frontend translates
  via `src/locales/{en,es}.json`.
- License is **GPL-3.0-only** (`LICENSE`, `docs/THIRD_PARTY.md`).

## 2. Engine architecture (V3, frozen)

```
[settings] -> os/obs.rs::write_obs_config(root, profile)
               root/obs-studio/basic/profiles/MoonClip/basic.ini
               root/obs-studio/basic/profiles/MoonClip/recordEncoder.json
               root/obs-studio/basic/scenes/MoonClip.json
               root/obs-studio/global.ini
               root/obs-studio/plugin_config/obs-websocket/config.json
            -> stage writable portable OBS copy at %LOCALAPPDATA%\MoonClip\obs
               (copy + portable_mode.txt + build marker; once per build)
               staged exe renamed to moonclip-obs.exe (unmistakable in TM)
                           --collection MoonClip --multi
                           --disable-shutdown-check --disable-updater
                           --only-bundled-plugins
            -> hide window (conceal_window watcher; no tray: user.ini
               SysTrayEnabled=false) -> config guard (root/obs-studio must
               appear, else abort)
            -> connect obs-websocket + `replay start`
F9         -> obs-websocket replay save -> poll `last-replay` -> DB index
```

- `root` = `%LOCALAPPDATA%\MoonClip\obs` — NEVER `%APPDATA%\obs-studio`.
- Websocket: `127.0.0.1:4456` (setting `obs_ws_port`) + generated password
  (`obs_ws_password`). MoonClip's in-process `obws` client is the only client.
- Encoder mapping (Windows, `os/windows/obs.rs`): NVENC
  `obs_nvenc_{h264,hevc,av1}_tex`, AMF `h264_texture_amf` / `h265_texture_amf`
  / `av1_texture_amf`, QSV `obs_qsv11_v2` / `obs_qsv11_hevc` /
  `obs_qsv11_av1`, CPU `obs_x264`.
- Capture source: `monitor_capture` (DXGI Desktop Duplication, `method: 1`,
  monitor index as string) or opt-in `window_capture` (WGC). **Never**
  `game_capture` (anti-cheat).
- Audio: `wasapi_output_capture` (loopback) + `wasapi_input_capture`; ids come
  from the WASAPI enumeration (`os/windows/devices.rs`);
  `default_output`/`default_input` map to OBS's `default`.

## 3. Module map (`src-tauri/src/os/`)

| File | Responsibility |
|---|---|
| `obs.rs` | shared engine: profile/scene writers, `ObsEngine`, config guard, log tail, tests |
| `obsws.rs` | obs-websocket v5 client (`obws`): replay/scene/source/video/input operations |
| `windows/obs.rs` | platform binding: portable-copy staging + marker, launch args, encoder ids, DXGI display source, orphan sweep |
| `windows/binary.rs` | resolve bundled `obs/bin/64bit/obs64.exe` (env override) |
| `windows/video.rs` | DXGI vendor, GDI monitor list (index + primary), ffmpeg-probed codec offer |
| `windows/devices.rs` | WASAPI endpoint enumeration + magic-default mapping |
| `linux/*` | mirrors (pactl devices, sysfs vendor, portal sources + RestoreToken, XDG_CONFIG_HOME isolation, KWin concealment, /proc orphan sweep) |
| `api.rs` | `CaptureConfig` + `CaptureEngine` trait (frozen surface) |
| `mod.rs` | per-OS selection + `new_engine()`, `resolve_obs`, `resolve_obscmd`, `obs_config_root`, `memory_free_mb` |

## 4. Audio (3 tracks, Mix first)

- Track layout via profile/scene: `RecTracks=7`, Track 1 `Master Mix
  [Game+Voice]` 320k, Track 2 `Game/Desktop` 320k, Track 3 `Microphone` 192k.
  Game source mixers `1|2`, mic `1|4`.
- Compatibility mode (`audio_single_track`) → `RecTracks=1`, both sources on
  track 1 only.
- Gains (0-200 %) are written into the scene (`volume`); changing them
  restarts the buffer once with a notice. Mutes apply live via
  `SetInputMute` over obs-websocket for `"MoonClip Game Audio"|"MoonClip Mic"`.
- `audio_peaks` returns `null` (meters not wired yet); the UI hides meters.
- Devices: `list_audio_devices` still enumerates via WASAPI.

## 5. Quality ladder and custom mode

- Shared `video_quality.rs`: Medal CBR ladder + `recommended_kbps` (Medal
  ranges, shown in the Video UI) + `ring_mb`.
- Settings keys: `video_codec` (h264/hevc/av1; legacy `x264` maps to h264+cpu),
  `video_encoder` (`gpu`|`cpu`), `gpu_index`, `out_height` (0=source),
  `fps` (24/30/60/120/144), `buffer_seconds`, `video_mode`
  (`ladder`|`custom`), `custom_bitrate_kbps` (3–100 Mbps), `custom_fps`,
  `custom_encoder_json`, `custom_video_json` (migration 008),
  `container`, `monitor`, `obs_ws_port`, `obs_ws_password`, gains/mutes.
- UI: `VideoSection.tsx` (Moon preset cards + Custom panel with per-encoder
  schema, codec-compat greying, sanitize-on-change, ladder-seeded bitrate,
  applied summary) + `NumberField.tsx` (shared numeric input: draft while
  typing, commit on blur/Enter, clamp+normalize) and `ObsEngineSection.tsx`
  (motor status + Repair). `SettingsModal` composes them.
- `RESTART_KEYS` in `commands.rs` restarts the buffer ONCE per change, with
  revert-on-failure and a notification; gains apply live first (restart only
  as fallback).

## 6. Commands (IPC surface)

- Capture: `start_buffer`, `stop_buffer`, `engine_status`, `save_clip_now`,
  `video_options`, `test_hardware`, `obs_info`, `repair_obs_config`.
- Audio: `list_audio_devices`, `audio_levels`, `audio_peaks` (null),
  `set_track_gain`, `set_track_mute`.
- Settings/clips/editor: unchanged (`get_settings`, `set_setting`,
  `set_settings`, `set_video_quality`, `list_clips`, `preview_track`,
  `open_clip_external`, secrets…).
- Removed: `gsr_info`, `fix_gsr_caps` (no GSR anymore).

## 7. Save path

1. `do_save_clip` → engine obs-websocket `replay save` + poll `last-replay`
   until the new path appears (45 s).
2. Same-second dedupe (`stem_2`, `stem_3`, …), size stat.
3. Duration probe + thumbnail in parallel (bundled FFmpeg).
4. SQLite insert (relative file name), ding, `moonclip://clip-saved`.

## 8. Acceptance (V3)

- [x] `cargo test` (36+ unit tests: profile/scene writers, parsers, encoder
      mapping, custom mode, Medal ranges, hotkey), `cargo clippy --all-targets
      -- -D warnings`, `pnpm build`, zero-`cfg` grep, `game_capture` grep.
- [ ] User passes: OBS del sistema abierto en paralelo (no se toca), F9 real
      con 3 pistas, cambio de preset/encoder/monitor con restart, Display
      capture en exclusiva, Vanguard/CS2 in-game, test de hardware desde el
      wizard, reparar OBS.
- [ ] Coexistencia visible: con el búfer corriendo NO hay icono de bandeja
      de MoonClip-OBS, NO hay ventana (ni parpadeo), y en el Administrador
      de tareas no hay un segundo "OBS Studio" suelto (el motor aparece
      como hijo de MoonClip / `moonclip-obs.exe`, nunca se confunde con el
      OBS del usuario).
- [ ] Legacy FSE (best effort; Display capture is compositor-level).

## 9. Test rig

- Buffer/save smoke: `pnpm tauri:dev`, Start, move the mouse, F9,
  `%LOCALAPPDATA%\MoonClip\Clips`.
- Tracks: `ffmpeg -i <clip>` shows 3 streams + titles; VLC → Audio → Track.
- Isolation: open the user's OBS, then MoonClip Start; both run, and
  `%APPDATA%\obs-studio` is untouched (compare mtimes).
- Anti-cheat grep: `git grep -n game_capture` must only show guard constants,
  comments and tests (never a generated ini/json string).
- Hardware test: Settings wizard → "Probar mi equipo" → result summary.

## 10. Packaging

- `src-tauri/tauri.windows.conf.json` bundles `obs/` and the FFmpeg sidecar
  as resources; `pnpm tauri:build:windows` produces NSIS + MSI.
- `build-aux/fetch-obs.ps1` pins OBS `32.2.2` (`OBS-Studio-32.2.2-Windows-x64.zip`,
  sha256 verified) and extracts the portable layout (the script never executes
  OBS; runtime validation happens through MoonClip's portable copy). Control is
  in-process over obs-websocket (`obws`), so no obs-cmd binary ships.
- SmartScreen applies to unsigned builds (see `08_CI_CD_DISTRIBUTION.md`).

## 11. Checklist before pushing

- [ ] No `sh`/`xdg-open`/`/proc`/`getcap`/`pkexec` on Windows paths.
- [ ] Clips live under `%LOCALAPPDATA%\MoonClip\Clips` by default (AV-safe).
- [ ] No hardcoded UI text (backend returns ids; locales cover EN+ES).
- [ ] No absolute paths in DB; relative names only.
- [ ] `game_capture` never generated; portable-copy + config guard intact.
- [ ] Behaviors above work without reshaping `commands.rs` IPC.
