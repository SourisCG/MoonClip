# 09 — Windows Handoff (V2, SPEC-driven)

Read this first, then `SPEC.md`, then `01_ARCHITECTURE.md` (rule 6 is law),
then `02_CAPTURE_ENGINE.md` (§3 is the Windows engine). The V2 rewrite
(2026-09-17) replaced the old `cpal` + `ts.rs` engine; the process
architecture was kept (decision "option A") because the SPEC's own §9/§11/§14
require a child ffmpeg with TS pipes/PES parsing.

## 1. How to work here

- Toolchain: MSVC (VS Build Tools) + Rust stable + Node 20 + `pnpm install`.
  Dev loop: `pnpm tauri:dev`. Verify: `pnpm build`,
  `cargo check --target x86_64-pc-windows-msvc`, `cargo test`,
  `cargo clippy --target x86_64-pc-windows-msvc -- -D warnings`.
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

## 2. Engine architecture (option A, frozen)

One child process owns capture + encode + mux:

```
ffmpeg -filter_complex "gfxcapture=hmonitor=<H>|ddagrab=output_idx=<n>
                        [,width/height/resize_mode=scale_aspect/scale_mode=bicubic],showinfo[out]"
       -map [out] -an -c:v <h264_nvenc|hevc_nvenc|av1_nvenc|h264_amf|hevc_amf|h264_qsv|hevc_qsv|libx264>
       <CBR ladder + §9 knobs> -r <fps> -fps_mode cfr
       -muxdelay 0 -muxpreload 0 -f mpegts pipe:1
```

- The TS goes over a custom 8 MB `CreatePipe` (`big_pipe`), never
  `Stdio::piped()` (32 KB ≈ 13 ms of slack at 20 Mbps; a drain hiccup used to
  freeze clips). Stderr gets 1 MB; drains run TIME_CRITICAL and parse only
  `showinfo` `pts_time` (pre-encoder clock).
- **Source cascade:** WGC (`gfxcapture`) is primary; if it delivers no frames
  or an all-black picture (startup `signalstats` probe, legacy exclusive
  fullscreen bypasses DWM), the engine respawns with `ddagrab` once. A
  mid-session stall surfaces `source_fallback_request()`; the liveness sweep
  restarts the buffer with the DXGI source and notifies the user.
- **Capture cap:** `min(refresh, max(2×fps, 90))`, fallback `max(2×fps, 120)`;
  `MOONCLIP_CAPTURE_MAX_FPS` overrides. Never the full refresh.
- **Scaling is live on the GPU** (bicubic zero-copy; lanczos on the download
  path), so saves are copy-only and `save_plan()` stays `None` on Windows.
- **Encoder recipe is the LIGHT default** (measured in-game): p5 + spatial AQ +
  BF2, `multipass disabled`, no look-ahead; AMF without `preencode`, QSV
  without `look_ahead`, x264 `veryfast+zerolatency`. The §9 heavy recipe
  (look-ahead / two-pass / pre-analysis) is opt-in via
  `MOONCLIP_ENCODER_HQ_FULL=1`; it starves capture under a GPU-saturating game.
- **Save I/O:** `+faststart` is a setting (default OFF: it rewrites the whole
  file); `capture_max_fps` (0 = auto 2×, clamped to refresh) trades motion
  sampling for GPU headroom. Stage telemetry per save:
  `mux: snapshot=… wav=… mux=…` + patch time.
- HDR is **out of V2**: capture is treated as normal SDR video (no color-space
  detection, no tonemap).

## 3. Module map (`src-tauri/src/os/windows/`)

| File | Responsibility |
|---|---|
| `detector.rs` | DXGI vendor, monitors + refresh + HMONITOR, probed `offered_codecs`, `capture_encoder_name`, `transcode_encoder` |
| `video.rs` | Source cascade, `capture_filter`, `capture_max_fps`, window/foreground helpers, `source_fallback` |
| `encode.rs` | §9 encoder args per vendor, NVENC preset env (`p5` default), `preset_step_down` (P4 when `video_lag > 0.8 s`) |
| `engine.rs` | SpawnPlan, child spawn + drains, frame proof, black probe, save/stop, priorities, orphan sweep, `CaptureEngine` impl |
| `ring.rs` | 1 MB chunked TS ring (O(1) trim), PAT/PMT, PES PTS/keyframes, two-phase `CutPlan` |
| `pts.rs` | Single QPC clock, `DEFAULT_SYNC_BIAS_MS=70` (+ env), `qpc_ticks_to_ns` for WASAPI |
| `audio.rs` | WASAPI loopback + mic threads, SPSC rings, keep-alive, watchdog, MMCSS |
| `dsp.rs` | f32→i16 conversion, gain, mix, peaks, QPC-grid `build_window` (fuse 50 ms) |
| `mux.rs` | Copy-only mux, §10 track layout, faststart, `alternate_group`, mp4/mkv naming |
| `devices.rs` | WASAPI endpoint enumeration/resolution (endpoint ids + legacy names) |
| `paths.rs`, `caps.rs`, `binary.rs`, `open.rs` | unchanged platform stubs |

## 4. Audio (WASAPI, SPEC §7/§8/§10)

- Loopback = render endpoint + `Direction::Capture` in shared mode (the crate
  sets `AUDCLNT_STREAMFLAGS_LOOPBACK`); mic = `eCapture`. The client is
  initialized as f32 48 kHz stereo with `autoconvert`.
- **One clock: QPC.** Each packet carries `BufferInfo::timestamp` (raw counter
  ticks → `pts::qpc_ticks_to_ns`); `build_window` rebuilds stems on the QPC
  grid and only gaps > 50 ms become silence.
- A **silent keep-alive render stream** keeps loopback delivering while the
  game is quiet; the supervisor follows the OS default device and reopens
  errors/stalls (3 s stall → one reopen per episode + 30 s slow retry).
- Track layout in the saved file (verified): 1 `Master Mix [Game+Voice]` AAC
  320k (default), 2 `Game/Desktop` 320k, 3 `Microphone` 192k, all in
  `alternate_group=1`; `audio_single_track` maps only the Master.
- Gains/mutes are applied in our path (atomics), never the OS mixer; peaks
  feed the UI meters.
- PID-isolated loopback (`AUDIOCLIENT_ACTIVATION_PARAMS`) is **not** in V2:
  left to the Linux owner (GSR) as documented in PROGRESS.

## 5. Quality ladder and custom mode

- `video_quality::bitrate_kbps` covers 360/480/720/1080/1440/2160 (Medal
  ladder, CBR; HEVC/AV1 rows), `cqp_export` 24/23/22/20/19/18 for offline.
- Custom mode settings: `video_mode` (`ladder`|`custom`),
  `custom_bitrate_kbps` (3 000–100 000), `custom_fps` (clamped to 30/60 in V1
  with a log/notice). Picking a ladder cell via `set_video_quality` exits
  custom mode.
- UI: `QualityTable.tsx` (moon presets, badges, duration/custom warnings) and
  `SetupWizard.tsx` (`settings.setup_done`). Moon names stay in English in
  both languages; only tags/descriptions are translated.
- Free-RAM warning: `system_memory` IPC (`os::memory_free_mb()`; Windows real,
  Linux `None` until wired by its owner).

## 6. Save path (copy-only)

1. `ring.cut_plan()` selects the latest keyframe ≤ `end − duration` and
   snapshots chunk handles (no payload copy under the lock).
2. `assemble()` materializes the staged TS outside the lock.
3. Mux over **stdin** (`-f mpegts -i pipe:0`), solo stems as WAVs, Master via
   `amix normalize=0`, `-c:v copy`, §10 audio args, `+faststart` for mp4,
   `-shortest`; then `patch_audio_alternate_group` rewrites the two `tkhd`
   bytes per audio track.
4. Telemetry per save: `capture: N real frames in T s = F fps` (last-10 s
   window) and `video_lag` (0.2–0.6 s is healthy; > 0.8 s logs the P4
   recommendation).

## 7. Acceptance (V2)

- [x] F9 → `.mp4` with h264 + 3×AAC (§10 titles/bitrates), save 0.23–0.29 s
      (1080p60, short buffer).
- [x] A/V flash+beep Game track ±8.33 ms; drift smoke −8.33 → +1.67 ms
      (min 1 → 3); inter-audio Master vs Game 0.00 ms; Master = clamped sum.
- [x] 120 s gold save 3.44 s / 289 MB on SATA (ceiling 5 s).
- [x] `ddagrab` override, x264 fallback, tone coverage 100 %.
- [x] `cargo clippy -D warnings`, 58 unit tests, zero-`cfg` grep, anticheat
      grep, `pnpm build`.
- [ ] User passes: BO7 10 min 1080p60/1440p60 (<3 % 1 % lows), 30-min drift,
      Kdenlive multitrack, legacy FSE (best effort), Linux CI regression.

## 8. Test rig (WGC delivers only on change)

Live video tests need screen motion:
- Mouse mover (background PowerShell moving the cursor in a loop) for plain
  buffer tests.
- `%TEMP%\opencode\playfull.ps1 <ref.mp4> <seconds>` (loops automatically)
  for the flash+beep reference; then `python %TEMP%\opencode\analyze_av.py
  <clip> 0:a:1` (Game) / `0:a:2` (Mic).
- `python build-aux/analyze_interaudio.py <clip>` (inter-audio + Master sum;
  `--selftest` validates the tool itself).
- Long runs: `MOONCLIP_DRIFT_SECS=<secs> cargo test … live_drift_capture
  -- --ignored` saves at ~1 min and at the end into `%TEMP%\moonclip-e2e-drift`.

## 9. Packaging

- `src-tauri/tauri.windows.conf.json` bundles the pinned ffmpeg sidecar
  (`binaries/x86_64-pc-windows-msvc/ffmpeg-…exe`) as a resource;
  `pnpm tauri:build:windows` produces NSIS + MSI. SmartScreen applies to
  unsigned builds (see `08_CI_CD_DISTRIBUTION.md`).
- `build-aux/fetch-ffmpeg.ps1` pins the BtbN build and asserts the
  `gfxcapture` filter plus the encoder set; re-run with `-Force` to bump.

## 10. Checklist before pushing

- [ ] No `sh`/`xdg-open`/`/proc`/`getcap`/`pkexec` on Windows paths.
- [ ] Clips live under `%LOCALAPPDATA%\MoonClip\Clips` by default (AV-safe).
- [ ] No hardcoded UI text (backend returns ids; locales cover EN+ES).
- [ ] No absolute paths in DB; relative names only.
- [ ] Behaviors above work without reshaping `commands.rs` IPC.
