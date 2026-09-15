# 09 — Windows Handoff (for the Windows-trip agent)

Read this first, then `SPEC.md`, then `01_ARCHITECTURE.md` (rule 6 is law),
then `02_CAPTURE_ENGINE.md`. Everything below is the full context needed to
implement the Windows side without breaking Linux.

## 1. How to work here

- Toolchain: MSVC (VS Build Tools) + Rust stable + Node 20 + `pnpm install`.
  Dev loop: `pnpm tauri:dev`. Verify: `pnpm build`,
  `cargo check --target x86_64-pc-windows-msvc`, `cargo test`.
- Minimum supported OS: **Windows 10 version 1903 (build 18362) or later**
  (Windows 11 supported). This is the WGC (Windows Graphics Capture) API
  floor — capture cannot work below 1903, so the installer targets 1903+.
- Package manager is **pnpm** (never npm). Commits: small, conventional
  (`feat/fix/docs`), push to `origin/main` when green.
- Zero-`cfg` rule: NO `cfg(target_os)` and NO OS APIs outside `src-tauri/src/os/`.
  Verify with the grep in `01_ARCHITECTURE.md` rule 6 — it must print nothing.
- IPC rule: `#[tauri::command]` auto-converts Rust `snake_case` params to
  **camelCase** wire keys (`clip_id` → `clipId`). Frontend ALWAYS sends
  camelCase. Never "fix" frontend keys to mirror Rust.
- No human text crosses IPC: backend returns ids/codes, frontend translates
  via `src/locales/{en,es}.json`. Keep it that way.
- License is **GPL-3.0-only** (see `LICENSE`, `docs/THIRD_PARTY.md`).

## 2. What Linux already does (mirror these behaviors exactly)

- Replay buffer with Start/Stop + status + F9 save (counter event always fires;
  clip saves only when running): `commands.rs` `start_engine`/`handle_hotkey`.
- 3-track layout order = track number: **1 = MIX** (game+mic, plays everywhere),
  2 = game only, 3 = mic only. Same container `.mp4`, AAC 160k.
- Medal CBR ladder per output height (same bitrate at 30 and 60fps):
  360p@3M, 720p@10M, 1080p@20M, 1440p@25M (h264; hevc/av1 rows in
  `video_quality.rs::bitrate_kbps`). Changing codec/height/fps/monitor/device
  with the engine running auto-restarts it (`RESTART_KEYS` in `commands.rs`).
- Per-track live gain/mute (0–200%, defaults 100) persisted in `settings`, applied live,
  re-applied on start, surfaced via `engine_status.tracks_linked` +
  `audio_error` (never fail silently).
- Save pipeline: flush ring → dedupe name (`stem_2.mp4`) → optional lanczos
  downscale → stat → probe real duration (`ffmpeg -i`) → thumbnail → DB
  `insert_clip` (relative paths only) → ding (`rodio`, synthesized) → event
  `moonclip://clip-saved` → notification with file name.
- Monitor selector (`-w <name>` on GSR; `monitor` setting, `""` = automatic).

## 3. Stub inventory — implement exactly these, same signatures

All in `src-tauri/src/os/windows/` (surface must stay identical to `os/linux/`):

| File | Implement with | Contract |
|---|---|---|
| `wgc.rs` | `windows-capture` crate (WGC/DXGI, Win10 1903+; NO DLL injection — anti-cheat safety) + NVENC/AMF/QuickSync HW encode | `CaptureEngine` trait in `os/api.rs` incl. `audio_args()` + `save_plan()`; `backend_name()` → real name |
| `audio.rs` | `cpal`: WASAPI loopback (`eRender` = game) + `eCapture` (mic) | `apply_gains(args, game, mic, mute_game, mute_mic)` multiplies samples in OUR path (never the OS mixer); `linked_count` reflects reality. GSR magic ids (`default_output`/`default_input`, seeded in settings) resolve to OS defaults. Privacy note: if Windows mic access is off for desktop apps (Settings → Privacy → Microphone), cpal links but captures silence — check there first on "no sound" reports |
| `devices.rs` | `cpal` device enumeration | Drop the `_bin: &Path` param (no sidecar on Windows); return `AudioDevice{id, description, kind}` with `kind` mic/desktop |
| `video.rs` | DXGI adapter query | `vendor()` → `nvidia`/`amd`/`intel`; `list_monitors()` → real `Monitor{name,width,height}`; `offered_codecs()` → subset of {h264,hevc,av1} the GPU encodes; `transcode_encoder()` already mapped (Nvenc/Amf/Qsv) |
| `binary.rs` | nothing | Keep returning the native-backend Err (by design) |
| `caps.rs` / `open.rs` | nothing | Already correct no-ops / `cmd /C start` |

Also: `host_triple()` in `sidecar.rs` already knows `x86_64-pc-windows-msvc`;
Phase 7 ships `ffmpeg` BtbN static as `ffmpeg-x86_64-pc-windows-msvc.exe`.

## 4. Acceptance (mirrors Linux, closes Phase 3)

- [ ] F9 in-game → `.mp4` <1s with h264 + 3×AAC, thumb visible, real duration.
- [ ] "N tracks linked" goes live by itself; sliders move real gains (audible).
- [ ] Mic/device dropdowns list real devices; selection persists + restarts buffer.
- [ ] Codec/resolution/fps/monitor changes apply with restart notice; ladder bitrates hold (`ffprobe`).
- [ ] `cargo check --target x86_64-pc-windows-msvc` + tests green; zero-`cfg` grep empty; Linux build still green (no regressions).

## 5. Checklist before pushing

- [ ] No `sh`/`xdg-open`/`/proc`/`getcap`/`pkexec` reachable on Windows paths.
- [ ] Clips never default under Videos/Documents/Desktop (AV-safe
  `%LOCALAPPDATA%\MoonClip\Clips` home + one-time legacy migration).
- [ ] No hardcoded UI text (backend returns ids; frontend locales cover EN+ES).
- [ ] No absolute paths in DB; `%LOCALAPPDATA%`-style locations resolve via `dirs`/`app_data_dir`.
- [ ] Behaviors in §2 all work without touching `commands.rs` contracts (extend, don't reshape, IPC shapes the frontend already uses).
