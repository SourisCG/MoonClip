# 02 — Capture Engine (Replay Buffer + Dual Audio)

## 1. Concept

A clip app does NOT record continuously to disk. It keeps already-encoded packets in a RAM FIFO and remuxes to `.mp4`/`.mkv` on hotkey (no re-encode).

- GPU zero-copy capture → hardware encode (NVENC / AMF / QuickSync / VA-API) → ring buffer in RAM → flush on shortcut.

## 2. Linux: `gpu-screen-recorder` (GSR) as sidecar daemon

GSR is the fastest path on Linux (X11 + Wayland, KMS/DRM direct, no portal dialog).

### 2.1 CLI (replay + triple audio)

```bash
gpu-screen-recorder \
  -w screen \
  -f 60 \
  -k h264 \
  -c mp4 \
  -r 30 \
  -a "default_output|default_input" \
  -a "default_output" \
  -a "default_input" \
  -o /home/user/Videos/MoonClip
```

Track layout (order = track number):
- **Track 1 = MIX** (`-a "desktop|mic"` merged): game + mic together, plays in any player / social embed. This is what gets shared.
- **Track 2 = game only**, **Track 3 = mic only**: solo stems for the Phase 5 editor.
- Merged `-a "x|y"` in any other position would merge there instead — keep the mix first.

The mix recording stream is named `gsr-combined-<random>` by GSR (proven in
source); solos are `gsr-<arg>`. Matching is exact against our `-a` args.

- `-w screen`: primary/focused monitor via KMS/NVFBC (no screencast dialog).
- `-r 30`: N-second RAM ring. `-o` is a **directory** in replay mode; GSR names files `replay_YYYY-MM-DD_HH-MM-SS.mp4`.
- `-a` (repeatable): Track 1 = MIX (`"desktop|mic"`), Track 2 = desktop/game, Track 3 = mic (AAC stereo each).

### 2.2 Video quality: Medal ladder + old-MoonLit NVENC HQ recipe

Bitrates are Medal's official recommended table (CBR). GSR runs
`-bm cbr -q <kbps> -tune quality -keyint 2`, plus NVENC HQ passthrough
on NVIDIA + h264/hevc only: `preset=p7;tune=hq;profile=high;bf=2;spatial-aq=1;multipass=disabled`
(all keys validated live against our bundled GSR: accepted, saves clean,
bitrate on target).

> **Windows trip:** same ladder + same bitrates, captured by the bundled
> FFmpeg `gfxcapture` filter (Windows.Graphics.Capture → D3D11 zero-copy)
> and encoded by NVENC/AMF/QuickSync (no GSR on Windows — see §3 and
> `09_WINDOWS_HANDOFF.md`). Windows scales on the GPU live, so the
> save-transcode ladder below does not apply there (`save_plan()` is `None`).
> Per-vendor save-transcode mapping lives in `os::video::transcode_encoder`
> (Nvenc/Amf/Qsv); VAAPI save-transcode on AMD/Linux is intentionally
> unmapped (render-node plumbing needs real-HW validation) → saver keeps the
> source file with a visible log, never silently.

| Height | H264 | H265 | AV1 | 60 s RAM (h264) |
|---|---|---|---|---|
| source | row of real height | … | … | … |
| 360p | 3M | 3M | 3M | ~23 MB |
| 720p | 10M | 7M | 7M | ~75 MB |
| 1080p | 20M (= old-MoonLit 20000 Kbps table) | 12M | 8M | ~150 MB |
| 1440p | 25M | 20M | 15M | ~188 MB |

Notes: 1080p@20M matches the old-MoonLit advanced table 1:1 (CBR/HQ/AQ/BF2/keyint-2s).
On Windows the default NVENC preset is **P5**, the preset OBS's own Auto
Configuration Wizard picks for this hardware (the user's OBS recipe is
otherwise identical). Measured on an RTX 3060 under COD with the real
zero-copy chain: P7 pulled only 19.6 fps with 852 dups / 20 s (~0.5x realtime),
P6 57.8 fps / 126 dups, P5 50 fps / 207. Remaining dups are the game's own fps
(a 60 fps CFR clip of a ~58 fps game has ~2 dups/s; OBS identical).
Linux GSR keeps the P7 HQ passthrough (parity with the validated table above).
`x264` (Windows CPU fallback, always listed) follows the h264 ladder row;
save-time it maps to `libx264` (`veryfast` + `zerolatency`).`preset=p7`/`profile=high` alone were tested and only work as part of the full
set above (alone they starve keyframes in tiny test buffers — an artifact of
short `-r`, not production buffers; the full set saves clean).
VAAPI/QSV/AMD keep GSR defaults (`very_high`, no passthrough opts).
FPS selector: 30 or 60 (default 60), same bitrate ladder for both — at 30fps
each frame gets ~2x the bits (same RAM per second, half the encoder load,
more judder from high-refresh sources). `-s` omitted at source resolution.
Same-second double saves never collide: the saver renames to `stem_2.mp4`,
`stem_3.mp4`… instead of overwriting + UNIQUE failure.
V2 adds the 480p and 2160p rows to the shared ladder (H264 5M / 60M,
HEVC 5M / 35M, AV1 5M / 25M) and `video_quality::cqp_export`
(24/23/22/20/19/18) for offline re-encodes; the buffer itself is always CBR.
Custom mode (`video_mode=custom`) uses the `custom_bitrate_kbps` slider
(3 000–100 000) and `custom_fps`, clamped to 30/60 in V1 with a notice.
The saved container follows the `container` setting (`mp4` default with
`+faststart`, or `mkv`).

### 2.3 Delivery strategy: source buffer + lanczos on save

Verdict from live A/B (same NVENC recipe sharp at 1080p native, soft at
720p on a 1:1 monitor): the backend's live scaler is soft on text at
non-integer ratios. So when the target height sits below the source height,
the ring buffer runs at **source** resolution (source-row CBR) and the saver
downscales with `scale=-2:H:flags=lanczos` (NVENC, target-row CBR) before
indexing. Direct capture otherwise. The Video UI shows the BUFFER cost and a
"records at source, delivers with lanczos" note whenever transcoding applies.
Monitor is explicit (`-w <name>`, default automatic); quality is a function of
settings, never of whichever monitor the backend finds first.

### 2.2 Control from Rust

- Spawn as `tokio::process::Child` on app start / game start.
- `save_clip()`:
  1. Snapshot the save time, then `nix::sys::signal::kill(pid, SIGUSR1)`.
  2. Poll every 100 ms (up to 5 s) for the `.mp4` whose mtime is >= the
     signal — a fixed sleep proved racy on slow disks / large rings, and
     "newest file" could return a previous clip; never falls back to one.
- `stop()`: `SIGINT` + `child.wait()`.
- `Drop`: `start_kill()` to avoid zombies.
- Stdio: GSR's stdout/stderr are drained by background readers into a bounded
  200-line ring. An unread 64 KB pipe buffer fills under load (GSR logs frame
  telemetry several times per second) and blocks GSR mid-capture.
- Liveness: `check_alive()` (`try_wait`) runs on the `engine_status` UI poll
  AND on a 2 s backend watchdog (the webview is paused while tray-hidden, so
  the UI poll alone cannot detect it during gaming). If GSR exited, the engine
  is dropped, the log tail surfaces in the UI and `moonclip://engine-stopped`
  fires + notification. No silent dead buffer.
- Codec ids: app `x264` = CPU encoding, spawned as `-k h264 -encoder cpu`
  (GSR rejects `h264_software` as a `-k` value; `--info` only reports it as a
  capability). NVENC HQ passthrough is skipped for x264 and save-scale uses
  `libx264` for it.

### 2.3 Permissions (avoid portal UX pain)

- Preferred: direct KMS capture, zero dialogs. Upstream GSR checks the cap on
  its KMS helper (`gsr-kms-server`, spawned next to the binary) and falls back
  to launching it via `pkexec` — an admin prompt on every capture start — when
  the cap is missing. Set it once at install/first run:
  ```bash
  sudo setcap cap_sys_admin+ep "$(dirname "$(which gpu-screen-recorder)")/gsr-kms-server"
  ```
  `os/linux/caps.rs` checks/fixes this same helper (one-click `pkexec` fix).
- Fallback: XDG Portal `ScreenCast` with `persist_mode=2` + saved `restore_token` + onboarding screen ("Pick Entire Screen → Check Remember → Share"). If stream metadata looks like a single window, warn the user.
- Audio discovery: `pactl list short sources` / `pw-dump`. `*.monitor` = output, others = inputs.

### 2.5 Live per-track gain (no editing, no monitoring side effects)

GSR has no volume flag, so gain is applied at the PipeWire layer:
each `-a` input is its own recording stream (source-output). Setting
`pactl set-source-output-volume <idx> <pct>` changes ONLY what GSR captures —
device volumes (what the user hears) are untouched. Same mechanism as
pavucontrol's Recording tab, which upstream itself recommends.

- Streams are found via `pactl -f json list source-outputs`, matched by
  `application.name` (~gpu-screen-recorder) and `media.name` (~monitor = game),
  with index-order fallback. Re-polled after spawn (streams appear async).
- Gains (`gain_game/gain_mic`, `mute_game/mute_mic`) persist in `settings`,
  apply live through `set_track_gain`/`set_track_mute`, and re-apply on every
  `start_buffer`. Range 0–200% (safe ceilings: game ≈100 — it already peaks
  near 0 dB at unity; mic ≈120–150 depending on the source; 200% is a boost
  tool for quiet sources, not a recommended level).
- Windows: gains persist identically; software multiplication lands on the
  WASAPI capture path (we own it there).

### 2.6 Deps (Linux)

```toml
[target.'cfg(target_os = "linux")'.dependencies]
nix = { version = "0.29", features = ["signal", "process"] }
tokio = { version = "1", features = ["process", "time"] }
rodio = "0.21" # confirmation ding (synthesized, no assets)
```

## 3. Windows: bundled FFmpeg `gfxcapture` (WGC) + WASAPI

- **OS floor: Windows 10 version 1903 (build 18362)+, Windows 11 supported.**
  WGC does not exist below 1903, so MoonClip for Windows requires 1903+;
  the installer targets that floor (see `08_CI_CD_DISTRIBUTION.md`).
- No DLL injection (anti-cheat safe for Valorant/CS2). The bundled FFmpeg
  captures through its `gfxcapture` filter — the same **Windows.Graphics.
  Capture** API Xbox Game Bar uses — and encodes **on the GPU**: frames stay
  in D3D11 textures (`AV_PIX_FMT_D3D11`) straight into NVENC/AMF/QSV. There
  is no WGC callback in the app process, no CPU readback and no rawvideo
  pipe: one child process owns capture + encode + mux. (`MOONCLIP_CAPTURE_
  SOURCE=ddagrab` switches the monitor source to Desktop Duplication as a
  fallback; window/app capture will use `gfxcapture`'s `window_title`/
  `window_exe`/`hwnd` selectors later.)
- Files (V2 rewrite, 2026-09-17): `os/windows/detector.rs` (DXGI vendor,
  monitors, probed codecs), `video.rs` (source cascade + filter graph),
  `encode.rs` (§9 args + presets), `engine.rs` (child + ring + save),
  `ring.rs` (MPEG-TS/PES index: PTS, keyframes, QPC calibration), `pts.rs`
  (single QPC clock + 70 ms anchor bias), `audio.rs` (WASAPI loopback + mic),
  `dsp.rs` (gain/mix/peaks/window builder), `mux.rs` (§10 container output),
  `devices.rs` (WASAPI endpoints). `cpal` and the old `ts.rs` are gone.
- Live chain (spawned at `start_buffer`):
  ```
  ffmpeg -loglevel info \
    -filter_complex "gfxcapture=hmonitor=<H>:max_framerate=<cap>:capture_cursor=1
                     [,width=W:height=H:resize_mode=scale_aspect:scale_mode=bicubic],showinfo[out]" \
    -map [out] -an -c:v h264_nvenc <CBR ladder + NVENC HQ> \
    -r <fps> -fps_mode cfr -muxdelay 0 -muxpreload 0 \
    -f mpegts pipe:1
  ```
  The TS goes over a custom 8 MB anonymous pipe (`big_pipe`), not
  `Stdio::piped()`: the default is 32 KB ~13 ms of slack at 20 Mbps, so any
  drain hiccup blocked ffmpeg and duplicated frames.
  Monitor selection is by **HMONITOR** (cross-process handle, validated) so
  the Rust-side DXGI list is authoritative. The buffer runs at the requested
  height: scaling happens inside the filter on the GPU (`resize_mode`), so
  saves are copy-only. `<cap>` is 2x the output rate clamped to the panel
  refresh (`capture_max_fps`; `MOONCLIP_CAPTURE_MAX_FPS` overrides) — under a
  game the full refresh would burn GPU copies the CFR filter discards.
  `-r <fps> -fps_mode cfr` pins the CFR timeline
  Medal/OBS style; `showinfo` logs each frame's PTS (100 ns, pre-encoder) on
  stderr.
- **A/V sync is two lines, no per-codec constants.** `ts.rs` parses PAT/PMT
  and every video PES (PTS in 90 kHz, unwrapped) and indexes keyframes
  (H.264 IDR / HEVC IRAP NAL scan; AV1 falls back to an `ffmpeg -ss` probe).
  The PTS clock is anchored to QPC with `min(stderr_arrival - pts)`, taken
  from `showinfo` **before the encoder**, so encoder lookahead (p7 + CBR can
  buffer ~0.5 s) cannot shift the mapping. At save, the chosen keyframe's QPC
  is `calib + key_pts` and both the video cut and the audio window start
  there. A rig-measured constant (`DEFAULT_SYNC_BIAS_MS = 70`, override
  `MOONCLIP_SYNC_BIAS_MS`) absorbs the compositor/frame-pool delivery lag
  that `showinfo` cannot see; it is not per-codec. Residual on the
  flash+beep rig: **median −3 ms, max ±13 ms**.
- **Audio (`audio.rs` + `dsp.rs`)** runs on the `wasapi` crate (0.24):
  endpoint loopback = render endpoint + `Direction::Capture` in shared mode
  (the crate sets `AUDCLNT_STREAMFLAGS_LOOPBACK`), mic = `eCapture`; the
  client is f32 48 kHz stereo with `autoconvert`. Each packet carries
  `BufferInfo::timestamp` (raw QPC ticks → `pts::qpc_ticks_to_ns`), so audio
  and video share one clock; `dsp::build_window` rebuilds each stem on the
  QPC grid (only gaps > 50 ms become silence). Capture threads push into
  lock-free SPSC rings; a silent keep-alive render stream keeps loopback
  delivering while the game is quiet; the supervisor follows the OS default
  device and reopens errors/stalls (3 s stall → one reopen per episode + 30 s
  slow retry). MMCSS "Pro Audio" on the capture threads.
- **Source cascade:** WGC (`gfxcapture`) is primary; a startup `signalstats`
  probe switches to `ddagrab` when WGC yields no frames or an all-black
  picture (legacy exclusive fullscreen). A mid-session stall requests the
  same switch through the liveness sweep (restart + notification).
- **Encoder load defaults:** the live recipe is the measured-light one
  (NVENC p5 + spatial AQ + BF2, single-pass, no look-ahead; AMF without
  `preencode`; QSV without `look_ahead`; x264 `veryfast+zerolatency`). The
  SPEC §9 heavy knobs are opt-in via `MOONCLIP_ENCODER_HQ_FULL=1`. The
  `capture_max_fps` setting (0 = auto 2× clamped to refresh) lets high-refresh
  panels trade motion sampling for GPU headroom. `+faststart` is a setting,
  default OFF (it rewrites the whole mdat on save).
- **Save is copy-only** (target <1 s): the ring index selects the latest
  keyframe at/before `end − duration`, the staged TS starts exactly at that
  PES with the latest PAT/PMT prepended (no `-ss` seek), WAVs for the solo
  stems are written in parallel and muxed with `-c:v copy` + the §10 layout
  (AAC 320/320/192, titles `Master Mix [Game+Voice]` / `Game/Desktop` /
  `Microphone`, `+faststart`, `patch_audio_alternate_group`).
- Robustness: the encoder child runs CPU class **HIGH by default**
  (`cpu_priority_class`; `MOONCLIP_CAPTURE_CPU_PRIO=0` restores ABOVE_NORMAL)
  with power throttling disabled and **GPU scheduling priority HIGH by default**
  (`tune_child_priority`; `MOONCLIP_CAPTURE_GPU_PRIO=0` disables) — the same
  lever OBS raises so WGC/NVENC do not starve behind a GPU-saturating game.
  Leftover capture children from a force-killed session are swept at
  `start_buffer` (`kill_orphan_ffmpeg`). A dead child/pipe stops the engine
  (`check_alive` + 200-line stderr tail); a source with no fresh PES for 3 s
  is reported as stalled (`log_tail`) instead of fabricating frozen frames.
- Per-track live gain/mute (0–200%) works exactly as on Linux: the gain is
  applied in our capture path (atomics read by the callback), never the OS
  mixer; `audio_peaks` publishes windowed peaks for the UI meters.


## 4. Rust trait (frozen interface)

```rust
// src-tauri/src/capture/mod.rs
use std::path::PathBuf;
use async_trait::async_trait; // or manual async in trait (Rust 1.75+)

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaptureConfig { pub duration_seconds: u32, pub fps: u32, pub output_dir: PathBuf }

#[async_trait]
pub trait CaptureEngine: Send + Sync {
    async fn start_buffer(&mut self, config: CaptureConfig) -> Result<(), String>;
    async fn save_clip(&mut self) -> Result<PathBuf, String>;
    async fn stop_buffer(&mut self) -> Result<(), String>;
    fn backend_name(&self) -> &'static str; // running state = Option<Engine> in AppState
}
```

Use `async-trait` if toolchain needs it; otherwise native `async fn` in traits.

## 5. Feedback (must-have for fullscreen)

- Audio cue: `rodio` plays embedded `clip_saved.wav` (~0.2s ding) after flush. WebView notifications are invisible in exclusive fullscreen.
- Optional overlay: secondary Tauri window (`transparent:true, decorations:false, alwaysOnTop:true, skipTaskbar:true`, `set_ignore_cursor_events(true)`), auto-hide after 2s. Linux caveat: layer-shell behavior varies on Wayland.

## 6. Acceptance (Phase 3)

- [ ] F9 in-game writes `.mp4` of last 30s in < 1s.
- [ ] File has 2 audio tracks (game + mic).
- [ ] Idle: no disk writes from capture; RAM ring only.
