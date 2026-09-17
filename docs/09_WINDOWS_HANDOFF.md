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
| `engine.rs` + `ts.rs` | Bundled FFmpeg **`gfxcapture`** (Windows.Graphics.Capture → D3D11 zero-copy → NVENC/AMF/QSV/x264) + `ts.rs` MPEG-TS/PES index (PTS, keyframes, QPC calibration) | `CaptureEngine` trait in `os/api.rs` incl. `audio_args()` + `save_plan()`; `backend_name()` → `"gfxcapture"`; no `windows-capture` crate, no pump, no rawvideo pipe |
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
- [ ] No absolute paths in DB; `%LOCALAPPDATA%`-style locations resolve via
  `dirs`/`app_data_dir`.
- [ ] Behaviors in §2 all work without touching `commands.rs` contracts (extend,
  don't reshape, IPC shapes the frontend already uses).

## 6. Engine invariants (2026-09-16 rewrite) — keep when touching `engine.rs`/`ts.rs`/`audio.rs`

The Windows engine was rewritten around the bundled ffmpeg `gfxcapture`
source (WGC, D3D11 zero-copy into the encoder). The old design (WGC callback
in-process, CPU readback, rawvideo pipe, CFR pacer, frame-stamp A/V) is gone.
What must stay true:

- **One child process owns capture + encode + mux.** `start_buffer` spawns
  `gfxcapture[,resize] → encoder → showinfo → -r <fps> -fps_mode cfr →
  -muxdelay 0 -muxpreload 0 → -f mpegts pipe:1`. Never reintroduce a Rust
  frame pump or CPU frame copies: they starved under game load and produced
  frozen clips (the pacer re-emitted one frame for minutes).
- **The encoder pipes are custom and deep.** `Stdio::piped()` is 32 KB
  (measured with `PeekNamedPipe`): at 20 Mbps that is ~13 ms of slack, so any
  drain hiccup blocks ffmpeg mid-capture and the CFR filter turns the gap into
  duplicated frames. `big_pipe()` (`CreatePipe`, inheritable write end only —
  an inherited read end would hide EOF) gives the TS stdout 8 MB (~3 s) and
  stderr 1 MB; our copies of the write ends are dropped right after spawn.
  Never go back to `Stdio::piped()`; never add `-flush_packets 1` (one syscall
  per packet, pointless with a deep pipe).
- **Monitor selection is by HMONITOR** (`gfxcapture=hmonitor=<value>`;
  `ddagrab=output_idx=<n>` for the `MOONCLIP_CAPTURE_SOURCE=ddagrab`
  fallback). HMONITOR handles are valid cross-process (validated). The
  DXGI monitor list is authoritative; never guess index order.
- **A/V sync = `calib + pts`, no per-codec constants.** `ts.rs` indexes video
  PES PTS (90 kHz, 33-bit unwrapped) and keyframes (H.264 IDR / HEVC IRAP;
  AV1 → `first_keyframe_secs` probe fallback). `calib` is
  `min(qpc - pts - sync_bias)` fed by **pre-encoder** `showinfo` lines
  (`note_clock_sample`), so NVENC lookahead (~0.5 s) can never shift it.
  - The PES header layout is `00 00 01 id | len(2) | '10' flags | PTS/DTS
    flags | header_data_length | PTS(5) [| DTS(5)]` — offsets 7/8/9. Getting
    this wrong silently corrupts PTS (that bug produced a 546 ms shift).
  - `DEFAULT_SYNC_BIAS_MS = 70` (override `MOONCLIP_SYNC_BIAS_MS`) absorbs
    the compositor/frame-pool delivery lag the log arrival cannot see; it is
    measured with the rig, NOT per codec. Re-measure after a compositor/GPU
    driver/monitor change (procedure below). Both `gfxcapture` and `ddagrab`
    need the same value.
  - Residual on the flash+beep rig: median −3 ms / max ±13 ms.
- **The clip starts exactly at its keyframe.** `cut()` slices the ring from
  the keyframe PES's byte offset and prepends the latest PAT/PMT; the muxer
  gets no `-ss` (input seek drops packets with PTS < target, which cut the
  IDR and left `start 0.001`). The pre-roll + `-ss` path exists only as a
  fallback when no PSI was parsed yet.
- **Audio windows are QPC grids.** `audio.rs` callbacks only convert + gain
  + push into lock-free SPSC rings (no allocs/locks on the audio thread);
  an assembler stores 10 ms i16 blocks with QPC; `build_window` resamples by
  block timestamps (drift correction) and emits silence only for gaps >
  `FUSE_NS` (50 ms). Never place blocks by absolute timestamps (per-block
  WASAPI jitter materialized as silence → static) and never fuse everything
  (up to 50 ms per gap compresses the timeline → drift).
- **Audio stream hygiene:** keep-alive silent render stream (loopback stops
  delivering when the engine idles), follow the OS default device for the
  magic ids, reopen errored/stalled streams (3 s stall, once per episode,
  30 s slow retry), windowed peaks for the UI. A stall episode is cleared
  **only by a delivered block** — `open_stream` must never reset
  `stall_handled`/`stall_retry_at`, and the watchdog always arms the 30 s
  slow retry after an attempt. The 2026-09-16 regression did reset them and
  produced ~840 "stalled, reopening" lines per minute plus one WASAPI stream
  per supervisor tick.
- **The video ring is chunked (O(1) trim).** `ts.rs` stores fixed 1 MB chunks
  in a `VecDeque` and trims by popping whole chunks. Never go back to a
  contiguous `Vec` + `drain`: at a 120 s buffer that memmoved ~326 MB under
  the mutex every ~0.4 s (measured 19-60 ms), stalling the encoder pipe in
  bursts (the "one frame every few seconds / 15 fps" report).
- **The capture rate cap is 2x the output rate, clamped to the panel refresh**
  (`engine.rs::capture_max_fps`: `min(refresh, max(2×fps, 90))`, fallback
  `max(2×fps, 120)`; `MOONCLIP_CAPTURE_MAX_FPS` overrides, clamped to >= fps).
  Never hardcode 60: WGC's `MinUpdateInterval` suppresses jittered presents
  (measured 256 vs 303 frames / 5 s at a 60 cap on a 165 Hz panel). Never run
  the full refresh either: under a game those extra WGC copies + `showinfo` /
  `fps`-filter work steal GPU time from NVENC while the CFR filter discards
  them. Two candidates per CFR slot (2x) means a suppressed present becomes a
  dropped excess frame instead of a duplicate. ffmpeg owns the exact CFR
  (`-r fps -fps_mode cfr`).
- **A/B capture sources** (env, useful under game load):
  `MOONCLIP_CAPTURE_SOURCE=ddagrab` (Desktop Duplication, `dup_frames=0`),
  `=window` (foreground HWND via WGC) or
  `MOONCLIP_CAPTURE_WINDOW_EXE='^cod.exe$'`. All three measured ~59.5-59.9
  fps real capture on the test rig. Monitor remains the default.
- **Save is copy-only and I/O-light** (`engine.rs::deliver`): the cut TS goes
  to the muxer over **stdin** (no ~300 MB staging write+read) and only the
  two solo stems are WAVs — the Mix is `amix=inputs=2:normalize=0` in ffmpeg
  (the sample sum the Rust path used to write to a third WAV). Video
  `-c:v copy`, 3×AAC 160k, `-shortest`, then `patch_audio_alternate_group`.
  The pre-roll fallback (no PSI parsed yet) keeps the staged-file path with
  per-input `-ss`. The cut itself is two-phase (`ts.rs::cut_plan` snapshots
  `Arc` chunk handles under the ring lock, `CutPlan::assemble` copies the
  bytes after releasing it): a ~300 MB save never stalls the encoder pipe.
  Never add a save transcode: the buffer already runs at the
  delivered height (GPU bicubic) and `save_plan()` stays `None` on Windows.
- **Telemetry to judge in-game health:** startup logs
  `capture buffer: WxH@fps (source, max <cap>)`; every save logs
  `capture: N real frames in T s = F fps` — a **last-10 s window** of
  pre-encoder clock samples (`CAPTURE_WINDOW_NS`), so CFR duplicates do not
  inflate it and idle desktop periods cannot mask in-game starvation (the old
  session average read 55 fps while the clip ran at ~30) — plus `video_lag`
  (should sit at ~0.2-0.6 s = encoder lookahead; growing means starvation).
- **Encoder/process hygiene:** child CPU class **HIGH by default**
  (`cpu_priority_class`; games run at HIGH and preempted our old
  ABOVE_NORMAL under load; `MOONCLIP_CAPTURE_CPU_PRIO=0` restores
  ABOVE_NORMAL, `normal` drops to NORMAL), power
  throttling disabled and cpal callbacks register MMCSS "Pro Audio". The
  **host process** also runs at ABOVE_NORMAL with power throttling disabled
  (`os/windows/mod.rs::prepare_environment`), and the engine's pipe-drain
  threads are TIME_CRITICAL (`boost_current_thread`): a parked drain thread
  used to fill a 32 KB pipe and ffmpeg blocks mid-capture (the in-game "one
  frame every few seconds" symptom); the 8 MB pipe above now absorbs seconds
  of drain latency. **GPU scheduling priority is HIGH by default**
  (`tune_child_priority` → `D3DKMTSetProcessSchedulingPriorityClass` from
  gdi32, level via `gpu_priority_level`): the same lever OBS raises. Without
  it, WGC capture + NVENC starve behind a game that saturates the GPU and the
  clip fills with duplicated frames; `MOONCLIP_CAPTURE_GPU_PRIO=0` disables,
  `2..=4` picks an explicit class. The kernel rejects the call with
  `STATUS_INVALID_PARAMETER` while the target has no live GPU context
  (measured: fails right after spawn, succeeds once ffmpeg's D3D11/NVENC
  device is up; GPU-less processes like ping always fail), so `spawn_encoder`
  retries it in the background (`retry_gpu_priority`, ~10 s) and logs the
  read-back. Leftover capture children from a
  force-killed session are swept at `start_buffer` (`kill_orphan_ffmpeg`:
  our exact binary path + dead parent; never PATH-resolved) — each leftover
  holds a WGC + NVENC session and only bites under game load. Test-only envs:
  `MOONCLIP_NO_SHOWINFO=1`, `MOONCLIP_NVENC_PRESET` (Windows default **p5**,
  the preset OBS's Auto Configuration Wizard picks: p7 measured 19.6 fps pulled
  / 852 dups per 20 s under COD on an RTX 3060; p6 57.8 / 126; p5 50 / 207 —
  probe recipe in PROGRESS) and `MOONCLIP_CAPTURE_BITRATE_KBPS` (CBR swap for
  A/B, < 500 ignored).
  `check_alive` = child alive + `video_dead`; the stderr drain keeps a
  200-line `log_tail` (never leave the pipe unread). **Hot-path rule:** the
  drain parses only `showinfo`'s `pts_time` line (pre-encoder clock) and
  discards every other `showinfo` line; only error-ish lines reach the
  console. No `-progress` (it wrote ~600 lines/s; the TS index owns
  telemetry now). A 3 s source stall is logged, never fabricated into
  frames.
- **Rig for A/V changes** (procedure): `pnpm tauri:dev` (or the live test)
  while `%TEMP%\opencode\playfull.ps1` plays `%TEMP%\moonclip-avtest\ref.mp4`
  fullscreen; save a clip covering the flashes; run
  `python %TEMP%\opencode\analyze_av.py <clip> 0:a:1` (stream 1 = Game).
  The reference itself must measure 0.00 ms. Player render skew is ±20 ms,
  so judge medians across runs. Re-run `live_buffer_and_save`,
  `live_engine_restart`, `live_tone_coverage` and
  `live_buffer_and_save_single_track` after touching the pump, the filter
  chain, the PES parser, the calibration or the mux.

Still pending (user pass): F9 in-game → clip <1 s under a heavy game, audible
gain sliders, device/monitor/codec switching with restart notice, and
default-track playback in the Windows 11 Media Player (Mix should sound; if
not, enable "Guardar solo el Mix" in Settings → Audio).
