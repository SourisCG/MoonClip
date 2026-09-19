# PROGRESS — MoonClip build log

Single source of truth for phase status. Updated at the end of every phase.
Details per phase live in `ROADMAP_PHASES.md`; technical specs in `01_*`–`08_*`.

| Phase | Scope | Status | Commit | Acceptance |
|---|---|---|---|---|
| 0 | Bare scaffold (Tauri v2 + React-TS + Tailwind v3) + `docs/` spec | ✅ done | `6965fac` (squashed in) | `pnpm build` + `cargo check` green |
| 1 | Tray + F9 hotkey + glass UI + starfield + i18n ES/EN | ✅ done | `6965fac` | F9 fires globally, tray hide/show works |
| 1-fixes | Frameless custom topbar + MoonLit CSS logo + smooth starfield + F9 dedupe | ✅ done | `8e4b5c1` | Single-count verified by user, no tray glitch |
| license | GPL-3.0-only (required by gpu-screen-recorder) | ✅ done | `2b99add` | Verbatim LICENSE + metadata + README |
| 2 | rusqlite persistence (relative paths) + keyring secrets + settings UI | ✅ done | `c72edba` | CRUD, vault OK, folder picker fixed |
| 3 | Capture engine Linux (GSR embedded, 3-track mix-first, gains, ladder, 30/60fps, monitor select) | ✅ done (Linux) | `5ffd70d`+ui | F9 → `.mp4` 3×aac, thumbs, durations, gains — user-verified |
| 3-win | Capture engine Windows trip (ffmpeg `gfxcapture` WGC 0-copy + WASAPI, AMF/QSV/x264, same behaviors) | ✅ done (code, HW-verified; user in-game F9 pass pending) | `7078a13`→rewrite | See `09_WINDOWS_HANDOFF.md`; e2e-tested on RTX 3060, A/V ±1 frame |
| 3-ui | Transparent tray icon, i18n codec labels, opener perms, disk note | ✅ done | `7c5d733` (batch) | user-verified pending |

## Cross-platform gate (project rule)

Any phase with per-OS code is implemented and tested on Linux first, then
**tested on Windows immediately before the phase is closed** — and likewise
in reverse whenever needed. No phase closes with an untested platform stub.
Applies from Phase 3 on (capture, detection, editor/FFmpeg, packaging).
| 4 | Game detection + launchers + custom apps | ⬜ pending | — | Native + Wine/Proton + Minecraft detected |
| 5 | Lazy editor + FFmpeg pipeline | ⬜ pending | — | Lossless <1s, vertical HW, no leak |
| 6 | Drive + social sharing | ⬜ pending | — | Public link copied + notified |
| 7 | CI/CD packaging | ⬜ pending | — | Tag produces all installers |

## Log

- **Windows: in-game lag + save slowness pass (2026-09-17)** — a 120 s clip
  saved from the field showed **~65 % duplicate frames** (mpdecimate: 2 563
  unique of ~7 300) and saves of **7–19 s** (SATA). Root causes and fixes:
  - **NVENC recipe was too heavy by default.** M2 had enabled the SPEC §9 full
    recipe (`-temporal-aq 1 -multipass qres -rc-lookahead 20`, `bufsize` 2×)
    on top of p5. The measured-light recipe is back as the default
    (`spatial-aq 1`, `multipass disabled`, no look-ahead, `bufsize` 1×); the
    heavy knobs are opt-in via **`MOONCLIP_ENCODER_HQ_FULL=1`**.
  - **Vendor equivalents aligned** (SPEC §9) in both modes: AMF light drops
    `preencode` (its pre-analysis/look-ahead; full adds `preencode 1 +
    preanalysis 1 + pa_taq_mode 2`), QSV light keeps `async_depth 4` (full adds
    `look_ahead 1 + look_ahead_depth 20`), x264 stays
    `veryfast + zerolatency`. `bufsize` is 1× for every vendor.
  - **Save I/O halved:** `+faststart` is now a setting (**default OFF**; it
    rewrites the whole mdat) and `patch_audio_alternate_group` no longer calls
    `sync_all()` inside the save path (it forced the OS to flush the whole
    ~300 MB file; measured ~2.4 s). New `capture_max_fps` setting (0 = auto
    2×, clamped to the panel refresh) to trade motion sampling for GPU
    headroom on high-refresh panels (164 Hz → 120 default; 90 is a good A/B).
  - **`dsp::build_window` hot loop optimized:** rational 20833+1/3 ns
    accumulator and i64 math, no division per sample (debug builds spent
    ~1.2 s on a 120 s window).
  - Stage telemetry per save: `mux: snapshot=… wav=… mux=…` + patch time.
  - **Measured (debug build, SATA):** 120 s save **3.08 s** (snapshot 1.18 /
    wav 0.64 / mux 0.62 / patch 0.57); short clips 0.33–0.41 s. Release
    builds cut the CPU stages further.
  - Pending: user in-game A/B (BO7) with `capture:` fps + `video_lag` lines,
    comparing Auto vs cap 90, and re-checking duplicates with mpdecimate.

- **Windows V2 rewrite — SPEC-driven (2026-09-17)** — the Windows engine was
  rebuilt milestone by milestone (M0–M8) against the V2 SPEC, keeping the
  proven process architecture (option A: one ffmpeg child owns
  capture+encode+mux) and replacing the rest. All gates green:
  `cargo clippy -- -D warnings`, 58 unit tests, `cargo check --target
  x86_64-pc-windows-msvc`, `pnpm build`, zero-`cfg` grep outside `os/`,
  anticheat grep empty.
  - **Architecture (decision, recorded):** option A. ffmpeg child with
    `gfxcapture` (WGC) / `ddagrab` (DXGI fallback) → NVENC/AMF/QSV/x264 →
    MPEG-TS over an 8 MB custom pipe → Rust ring/PES index → copy-only save.
    The in-process `windows-capture` + `ffmpeg-next` option B was discarded
    because the SPEC's own §9/§11/§14 require the child-process pipes/PES path
    and the FFI risk is not justified by the measured results.
  - **New module layout** (`os/windows/`): `detector.rs` (DXGI vendor,
    monitors+refresh, probed `offered_codecs`), `pts.rs` (single QPC clock,
    `DEFAULT_SYNC_BIAS_MS=70`, `qpc_ticks_to_ns` for WASAPI), `ring.rs`
    (1 MB chunks O(1), PES/keyframes, two-phase `CutPlan`), `encode.rs` (§9
    per-vendor args, NVENC p5 + auto-P4 recommendation when `video_lag>0.8 s`),
    `video.rs` (source cascade + filter graph + `source_fallback`),
    `dsp.rs` (conversion/gain/mix/peaks/`build_window`), `audio.rs` (WASAPI),
    `mux.rs` (copy-only mux, §10 tracks, faststart, alternate_group, mp4/mkv).
    `cpal` and the old `ts.rs` are gone.
  - **Audio:** `wasapi 0.24` (endpoint loopback = render + `Direction::Capture`,
    `BufferInfo::timestamp` = raw QPC ticks), SPSC lock-free rings, silent
    keep-alive render stream, default-device follow, stall watchdog without
    churn, MMCSS. Track layout §10 verified inside the file: `name` metadata
    `Master Mix [Game+Voice]` / `Game/Desktop` / `Microphone`, AAC 320/320/192,
    `-movflags +faststart` (moov at byte 36), `alternate_group=1`.
  - **Ladder/§17 backend:** `video_quality` now covers 360/480/720/1080/1440/
    2160 with CQP export 24/23/22/20/19/18; custom mode settings
    (`video_mode`, `custom_bitrate_kbps` 3–100 M, `custom_fps` clamped to
    30/60 with notice); atomic `set_settings` (one restart) and
    `set_video_quality` (codec+height+fps+exits custom); `system_memory`
    (free RAM for the warning; Windows `GlobalMemoryStatusEx`, Linux `None`
    until its owner wires it).
  - **UI §17:** `QualityTable.tsx` (moon presets `New Moon 720p / First
    Quarter 1080p / Waning Gibbous 1440p / Full Moon 2160p`, EN names in both
    languages, tags `bajo/recomendado/alto/ultra` · `low/recommended/high/
    ultra`, AV1 only when offered, badges, duration 15/30/60/120 s + 5/10 min
    + free custom with >2 min risk checkbox and free-RAM warning, container
    MP4/MKV, hover preview) and `SetupWizard.tsx` (first run via
    `settings.setup_done`, NVIDIA suggests 1080p, otherwise 720p; reopenable
    from Settings). `VideoSection` keeps the monitor selector and the KMS
    banner; the old codec/resolution/fps selects were replaced.
  - **Decisions logged:** HDR is OUT of V2 (capture stays SDR/normal video);
    PID-isolated loopback is OUT of Windows V2 and left to the Linux owner
    (documented here); `QualityTable` is shared UI (Linux backend maps 480 via
    save-lanczos and 2160 at source until its owner aligns GSR); the
    `buffer_seconds` 300 s cap is gone (only ≥5 s is enforced; >120 s is a UI
    warning); default NVENC preset stays **p5** with the documented P4
    step-down recommendation.
  - **Measured (RTX 3060, 1080p60, SATA SSD C:):** live WGC+NVENC+WASAPI save
    0.23–0.29 s; A/V flash+beep Game track **+8.33 ms**; drift smoke at
    minute 1/3: **−8.33 / +1.67 ms** (no accumulation); inter-audio
    Master vs Game **0.00 ms** and Master vs clamped sum **0 exact**;
    `live_tone_coverage` 100 %; 120 s gold save **3.44 s / 289 MB** (SATA
    ceiling is 5 s; <3 s is the NVMe target); `ddagrab` source override and
    x264 fallback save 3×AAC.
  - **Test rig note:** WGC/DDA deliver frames only on change, so live video
    tests need screen motion. PROGRESS rig: a background PowerShell mouse
    mover (cursor positions in a loop) plus `playfull.ps1 <ref.mp4> <secs>`
    (loops automatically) for the flash+beep reference.
  - **Tooling:** `build-aux/analyze_interaudio.py` (pure Python FFT
    cross-correlation, `--selftest`, Master-vs-sum check) and the existing
    `analyze_av.py` / `playfull.ps1` rig.
  - **Packaging:** `src-tauri/tauri.windows.conf.json` bundles the pinned
    ffmpeg sidecar as a resource; `pnpm tauri:build:windows` builds NSIS+MSI
    (SmartScreen note applies; installer build still to be smoke-tested).
  - **Pending user passes (M7 manual):** BO7 10 min at 1080p60 and 1440p60
    (<3 % 1 % lows, `video_lag` 0.2–0.6 s), 30-min drift
    (`MOONCLIP_DRIFT_SECS=1800 cargo test … live_drift_capture`), Kdenlive
    multitrack (3 stems + labels), legacy FSE (best effort; BO7 uses
    borderless/eFSE), and the Linux regression (`cargo check` + `pnpm build`
    on Linux CI).

- **Windows: in-game probe on COD — P7 is the real ceiling (2026-09-16)** —
  isolated probes while COD (foreground) ran, no app:
  | Probe (20 s, real path unless noted) | Frames pulled | Dups |
  |---|---|---|
  | WGC capture only (`wrapped_avframe`) | 1028 (51 fps) | — |
  | DDA capture only (`ddagrab dup_frames=0`) | 516 (26 fps) | — |
  | WGC → P7 → CFR 60 | **392 (19.6 fps)** | **852** |
  | WGC → P6 → CFR 60 | 1155 (57.8 fps) | 126 |
  | WGC → P5 → CFR 60 | 1000 (50 fps) | 207 |
  | WGC → P1 → CFR 60 | 964 (48 fps) | 237 |
  | testsrc2 → P7 → null (encoder only) | 900 in 28 s | speed 0.526x |
  - Capture is fine (WGC ~51 fps; DDA worse here, so WGC stays default);
    **NVENC P7 collapses under game load** (~0.5x realtime; the CFR filter
    then emits 852 duplicates per 20 s).
  - **Windows default preset is now p5** (the preset OBS's own Auto
    Configuration Wizard picked for this hardware; env
    `MOONCLIP_NVENC_PRESET` swaps it: `p4` more headroom, `p7` Linux parity).
    Same ladder, same CBR, same HQ/AQ/BF2 as the user's OBS. Note: the earlier
    "OBS P7" screenshot was *MoonLit* (the user's old app), not OBS; OBS's
    wizard recommends P5, which resolves the contradiction.
  - **Final config: p5 @ 20000 kbps** (Medal ladder). A live p6 + 10000 kbps
    pass was run through the app (`MOONCLIP_CAPTURE_BITRATE_KBPS` test hook)
    and discarded: 10 Mbps is the wizard's *streaming* number, and the user
    prefers the full 20 Mbps ladder.
  - Gate: 47 unit tests green (p5 assert + bitrate-override parse test).

- **Windows: encoder pipe depth + CPU class + windowed telemetry (2026-09-16)** —
  measured the real stdout pipe with `PeekNamedPipe`: **32 KB**, i.e. ~13 ms of
  slack at 20 Mbps. Any scheduling hiccup in our drain threads blocked ffmpeg
  mid-capture and `-r 60 -fps_mode cfr` filled the gap with duplicates — a
  mechanism that only shows under game CPU/GPU load. Fixes (preset untouched
  in this pass: the later in-game probes found p7 collapses under COD — see the
  entry above — and the Windows default was moved to p5):
  - `engine.rs::big_pipe()`: `CreatePipe` with an 8 MB TS stdout buffer
    (stderr 1 MB), inheritable write end only; parent-side write ends dropped
    right after spawn so a dead ffmpeg still surfaces as EOF. `-flush_packets
    1` removed (one syscall per packet, pointless with the deep pipe).
  - `cpu_priority_class`: encoder child CPU class **HIGH by default**
    (`MOONCLIP_CAPTURE_CPU_PRIO=0` → ABOVE_NORMAL, `normal` → NORMAL).
  - `ts.rs`: `capture:` telemetry is now a **last-10 s window**
    (`CAPTURE_WINDOW_NS`) instead of a session average; the old average read
    55 fps while the saved clip ran at ~30 because idle desktop periods
    masked in-game starvation.
  - Gate: 46 unit tests + `live_buffer_and_save{,_single_track}` green
    (saves 0.35 s, GPU priority applied, sync telemetry sane).
  - Pending user in-game pass (camera movement) + `ddagrab` A/B: OBS display
    capture defaults to DXGI Desktop Duplication, which is our `ddagrab`
    fallback (`MOONCLIP_CAPTURE_SOURCE=ddagrab`).

- **Windows: in-game GPU budget pass — priority, cap, orphan sweep (2026-09-16)** —
  saved clips still showed ~25-34 unique fps under game load (measured 2612
  exact duplicates / 7215 frames = 58% on a 120 s clip; 9-50 fps per 10 s
  window) while the same chain on the desktop kept 58.75 fps unique with only
  **3.4% realtime headroom** (`speed=0.966x`, p7+HQ 1080p60 CBR, RTX 3060) and
  the game itself stayed smooth (solo el clip sale mal). Root cause: GPU
  scheduling starvation — with the game saturating the GPU, the WGC copy stops
  getting fresh frames and `-r fps -fps_mode cfr` fills the gaps with
  duplicates. OBS/Medal survive because they capture at 60 and raise the GPU
  scheduling class; we captured at the full 164 Hz refresh with that lever
  off. Fixes (A/V pipeline untouched):
  - **GPU scheduling priority HIGH by default** for the ffmpeg child
    (`tune_child_priority` + `gpu_priority_level`; `MOONCLIP_CAPTURE_GPU_PRIO`
    unset = HIGH(4), `0/off/false` disables, `2..=4` picks a class, `1` stays
    the legacy alias). dxgkrnl rejects the call with `STATUS_INVALID_PARAMETER`
    while the target has no live GPU context (measured: fails right after
    spawn; the same call on the bundled ffmpeg mid-NVENC succeeds; GPU-less
    processes like ping/cmd always fail), so a background retry
    (`retry_gpu_priority`, ~10 s) applies it and the read-back is logged
    (`applied after 250ms (read_back=4)` on the dev rig).
  - **Capture cap = 2x output, clamped to refresh** (`capture_max_fps`): 164
    Hz → 120 (was 164, not the pathological 60) so the CFR filter always has
    two candidates per slot; `MOONCLIP_CAPTURE_MAX_FPS` overrides.
  - **Orphan ffmpeg sweep** (`kill_orphan_ffmpeg`, called at `start_buffer`):
    a force-killed host left its capture+encode child running (seen live with a
    dead parent); leftovers stack NVENC sessions and only bite in-game. Only
    the exact bundled binary with a dead parent is killed.
  - **Two-phase save cut** (`ts.rs::cut_plan` + `CutPlan::assemble`): the
    ~300 MB ring snapshot copies `Arc` chunk handles under the lock and the
    payload outside it, so a save can no longer stall the encoder pipe.
  - Gate: `cargo check` + 44 unit tests green (2 new: cap headroom, GPU
    priority env mapping). Pending user in-game pass: `capture: F fps` >= 55
    and no freeze > 100 ms in the saved clip.

- **Windows: process/thread hardening + in-game isolation rig (2026-09-16)** —
  the user's in-game capture still stalled to ~1 frame every 2-3 s while
  OBS/Medal *display capture* works focused on the same machine, so the
  compositor is not the blocker; our host process is. Root suspects: the host
  had no priority class and no anti-throttling (only the ffmpeg child did),
  and both engine pipes (TS ~2.5 MB/s and `showinfo` ~50 KB/s, 64 KB each)
  were drained by default-priority threads — if a drain is parked while a
  focused game owns the CPU, ffmpeg blocks mid-capture in multi-second bursts.
  - `os/windows/mod.rs`: `prepare_environment()` now sets the host to
    ABOVE_NORMAL and disables power throttling (EcoQoS); new
    `boost_current_thread()` (TIME_CRITICAL) called by the TS and stderr
    drains.
  - GPU scheduling priority is now **opt-in** (`MOONCLIP_CAPTURE_GPU_PRIO=1`):
    it may preempt the game instead of helping.
  - Test envs: `MOONCLIP_NO_SHOWINFO=1` (drop pre-encoder logging; A/V falls
    back to the coarser PES-arrival calibration) and
    `MOONCLIP_NVENC_PRESET=p5` (NVENC load A/B; default stays p7 = Linux
    quality).
  - Isolation rig `%TEMP%\opencode\capture-diag.ps1`: runs the exact engine
    ffmpeg chain (no app) for 4 variants in-game (wgc p7±showinfo, wgc p5,
    ddagrab) so the bottleneck can be attributed from the saved `.ts` files;
    validated on the desktop at 59-60 fps real for all variants.
  - Gate: 42 unit tests, live save tests green, saves 0.17-0.21 s.

- **Windows: capture-rate, ring and save-speed pass (2026-09-16)** — with the
  log flood gone, in-game clips measured 26-41 unique fps with bursty
  seconds (p25=9, p75=44) while a 60 fps source captured at ~49 fps with an
  idle GPU. Four self-inflicted causes fixed, quality untouched (p7/HQ/ladder
  identical to Linux):
  - **Chunked ring (O(1) trim).** The 120 s buffer (327 MB) memmoved ~326 MB
    under the mutex every ~0.4 s (measured 19-60 ms), stalling the encoder
    pipe. `ts.rs` now stores 1 MB chunks; cross-chunk packets are assembled
    188 bytes at a time.
  - **No more 60 cap.** `max_framerate` = the monitor's real refresh (DXGI +
    `EnumDisplaySettingsW`, fallback `max(120, 2×fps)`); ffmpeg owns CFR.
    Measured end-to-end: **59.9 fps real capture** vs ~49 before.
  - **HIGH GPU scheduling priority** for the ffmpeg child
    (`D3DKMTSetProcessSchedulingPriorityClass`) plus the existing
    ABOVE_NORMAL CPU class, so a saturated game cannot starve capture.
  - **Fast save:** cut TS over stdin (no ~300 MB staging write+read) and Mix
    built by `amix normalize=0` instead of a third WAV. Mux 0.17-0.25 s on
    10-16 MB clips; on the user's old SATA SSD the 2 min case should drop
    from ~12 s to ~4-6 s.
  - A/B sources via env (all ~59.5-59.9 fps on the rig):
    `MOONCLIP_CAPTURE_SOURCE=ddagrab|window`, `MOONCLIP_CAPTURE_WINDOW_EXE`.
  - New telemetry: `capture: N real frames in T s = F fps` per save.
  - Gate: 42 unit tests (chunk-boundary packets, trim, filter shapes), 5 live
    tests, A/V rig **+10 ms**, `pnpm build`, zero-cfg grep.

- **Windows: capture freeze root-caused (log flood + mic reopen churn, 2026-09-16)**
  — the user reported "one frame every few seconds" in-match. Evidence: the
  in-match clips had 2-4 unique frames per 120 s (mpdecimate/freezedetect)
  while lobby/desktop clips had 1.2-4.4k, and a controlled 60 fps source on
  the same monitor captured at ~51 fps — so WGC/GPU were fine. The console
  showed the real cause: `open_stream` reset the stall episode, so the
  watchdog reopened the mic every tick (**840 "stalled, reopening" lines and
  one WASAPI stream per tick**), and every non-`pts_time` `showinfo`
  continuation line was `eprintln!`-ed (~300/s) plus `-progress` (~600/s).
  A blocked console stalls the stderr drain, ffmpeg's 64 KB stderr pipe
  fills and the filtergraph blocks mid-capture: duplicates-only clips.
  - `audio.rs`: stall invariants restored (only a delivered block clears the
    episode; `open_stream` never resets it; the supervisor always arms the
    30 s slow retry), with a pure `stall_action()` + regression test.
  - `engine.rs`: all `showinfo` lines except `pts_time` are discarded, only
    error-ish lines reach the console, `-progress`/`frames_encoded` removed
    (stderr is now ~60 lines/s).
  - Verified: 42 unit tests, 5 live tests, and an end-to-end motion capture
    through the exact engine chain gave **273 unique frames in 5.6 s (~49
    fps)** where the same setup previously produced 7; saves 0.26-0.62 s;
    `video_lag` ~0.2-0.6 s (encoder lookahead) with no stalls.
  - Pending user in-game pass with the fixed build.

- **Windows capture engine rewritten around FFmpeg `gfxcapture` (2026-09-16)** —
  the old engine (WGC callback in-process → CPU readback → rawvideo pipe →
  swscale → NVENC, CFR pacer, frame stamps) froze under game load: two user
  clips were a single frame repeated for 2 minutes with all three audio stems
  at −91 dB. Root causes: ~500 MB/s of CPU copies + software conversion, a
  4-frame queue that dropped every frame while `write_all` was blocked (the
  pacer then re-emitted the last frame for minutes), and audio callbacks that
  allocated/locked and could be starved.
  - New `os/windows/engine.rs`: one child process owns capture + encode +
    mux — `gfxcapture=hmonitor=<H>(,resize GPU bicubic),showinfo → enc →
    -r fps -fps_mode cfr → mpegts`. Frames never leave the GPU; saves are
    copy-only. Measured ~8-11% of one core for 1080p60 vs ~26% for the old
    conversion alone, plus ~0.5 GB/s of pipe traffic. `windows-capture` dep
    removed (DXGI monitor/vendor discovery via `windows`); the 4th-session
    ACCESS_VIOLATION class of bug goes with it. `ddagrab` fallback via
    `MOONCLIP_CAPTURE_SOURCE`; app/window capture will reuse `gfxcapture`'s
    window selectors later.
  - New `os/windows/ts.rs`: MPEG-TS/PES index (PTS 90 kHz unwrapped, H.264
    IDR / HEVC IRAP keyframes, PAT/PMT for exact staging) + QPC calibration
    from pre-encoder `showinfo` lines. Encoder lookahead (~0.5 s) no longer
    shifts A/V. PES header offsets bug caught by tests (flags at 7/8/9).
  - `os/windows/audio.rs` rewritten: lock-free SPSC from the callbacks (no
    allocs/locks), MMCSS "Pro Audio", i16 timestamped 10 ms blocks, QPC-grid
    window builder with drift resampling (only >50 ms gaps become silence),
    keep-alive + default-follow + stall reopen kept.
  - Save: latest keyframe ≤ `end − duration`, staged TS starts at the
    keyframe with PAT/PMT prepended (no `-ss`, no probe, no decode), WAVs in
    parallel, `-c:v copy` + 3×AAC, alternate-group patch. Fixtures:
    save 0.3-0.45 s (debug build), video starts at 0.
  - A/V with the flash+beep rig: **median −3 ms, max ±13 ms** (was −546 ms
    after the first rewrite attempt). The only constant is
    `DEFAULT_SYNC_BIAS_MS = 70` (override `MOONCLIP_SYNC_BIAS_MS`), a
    rig-measured compositor/frame-pool delivery lag — same for `gfxcapture`
    and `ddagrab`, not per codec.
  - Gate: 41 unit tests (new TS/keyframe/window/SPSC tests), 5 live tests
    (`live_buffer_and_save{,_single_track}`, `live_engine_restart`,
    `live_tone_coverage`, `live_av_offset_capture`), `pnpm build`, zero-cfg
    grep. Docs 01/02/09 + README updated; `fetch-ffmpeg.ps1` now asserts the
    pinned build ships `gfxcapture`.
  - Pending user pass: F9 in a heavy game (no freezes, <1 s save, RAM),
    audible gain sliders, device/monitor/codec restart notices.

- **A/V: audio window anchored at the CUT instant (2026-09-16)** — the
  frame-stamp anchor (`-progress frame=`) was read **after** the two ffmpeg
  probes (duration + keyframe), so it counted frames encoded while probing
  that are NOT in the clip: the audio window stretched and h264 NVENC HQ
  stayed ~0.1 s off (previous measurement: −28/−80/−100/−127 ms by codec).
  Fix: `save_clip` snapshots `frames_encoded` + the newest capture stamp
  **inside the ring-cut critical section** (`cut_encoded`/`cut_stamp`) and
  maps the audio window end from that value; the probes below never touch
  the anchor. Never move the anchor read back after the probes.
  - Re-measured with the flash+beep reference and a browser-free rig
    (fullscreen WPF `MediaElement` player + `analyze_av.py`, which validates
    0.00 ms on `ref.mp4`): h264 **−15 / +2 / +28 ms** across runs (players
    add ±20 ms of their own render skew), hevc **−8 ms**; before the fix the
    same rig shows the ~0.1 s stretch.
  - Regression green: `live_buffer_and_save` (3×AAC + alternate_group),
    `live_buffer_and_save_single_track` (1×AAC), `live_engine_restart`
    (keyframe trim + duration) and 35 unit tests.

- **Default audio track: MP4 `alternate_group` + "Mix only" compatibility mode (2026-09-16)**
  - The 3-track clip already complied (Mix = first audio + only `enabled`/
    default; solos `disabled`, titles `Mix/Game/Mic`), but the Windows 11
    Media Player ignored the flags and picked another track. ffmpeg cannot
    write `alternate_group` from the CLI (muxer side-data only), so
    `wgc.rs::patch_audio_alternate_group` patches the `tkhd` of every audio
    track (`soun` handler) to group `1` **in place** after the mux: moov read
    into memory, mdat seeked over, only the 2 u16 fields rewritten. Best
    effort: a failure logs and the clip is kept. Unit tests cover both tkhd
    versions, the byte-diff (video untouched, nothing else changed) and the
    no-audio case; `live_buffer_and_save` now asserts 3 tracks / one group /
    only the first enabled.
  - New setting **`audio_single_track`** ("Guardar solo el Mix (1 pista)",
    Settings → Audio, default OFF): the mux maps only `0:v` + `1:a`, so any
    player plays the Mix because there is nothing else. Restarts the buffer
    like the other capture settings (the running engine holds the flag);
    capture/A-V code untouched. `CaptureConfig` carries the flag; new live
    test `live_buffer_and_save_single_track` asserts 1×AAC.
  - Gate: 35 unit tests + `live_buffer_and_save{,_single_track}` +
    `live_engine_restart` green (`alternate_group=1 on 3/1 audio track(s)`),
    `pnpm build` green.

- **Windows audio: keep-alive + stall-churn fix + frame-stamped A/V alignment (2026-09-16)**
  - Root cause of "only the voice OR the PC audio" and the missing desktop
    audio: the stall watchdog reopened the game loopback every ~0.5 s forever
    (`game stream stalled … reopening` … in the user console). WASAPI delivers
    no packets while the render engine is idle (normal), the watchdog took it
    for a dead stream, and `clear_stream_error` cleared the "handled" flag on
    every reopen → infinite churn. One session's game stem ended with
    `silence=33.7s` inside a 31.9 s window. Fixes: a **silent keep-alive
    render stream** on the captured endpoint (the engine never sleeps, the
    loopback delivers silence continuously → 100% coverage), the stall flag is
    only cleared by a real delivered block, a 30 s slow retry per episode and
    a grace period after each reopen.
  - A/V alignment rewritten to OBS/Game Bar semantics **without constants**:
    `Frame::timestamp()` (WGC, QPC 100 ns) travels with every frame through
    the pacer, which records `(frame_index, capture_qpc)`; ffmpeg's
    `-progress frame=` tells how many frames the encoder has emitted, so at
    save the audio window ends exactly at the capture instant of the last
    encoded frame (`-stats_period 0.02`). Audio blocks carry QPC as well:
    one clock for everything, no cross-domain mapping. The 258/264/48 ms
    per-codec constants are gone.
  - Measured with the flash+beep reference clip (Edge kiosk + local ffmpeg
    pattern): residual **−28 ms (hevc), −80 ms (x264), −100/−127 ms (h264
    NVENC HQ, two runs)** vs ±1.5 s before — every codec and run lands within
    ~0.1 s with zero tuning. Tone tests: 100% coverage, zero stalls/reopens.
  - Full gate: 33 unit tests, 2 live tone tests, 3 live A/V captures green;
    `pnpm build` green.

- **Windows A/V sync measured and fixed (2026-09-16)** — the remaining
  "audio is out of sync" report was quantified with a **flash+beep reference
  video** (generated with the embedded ffmpeg: 100 ms white flash + 1 kHz
  beep every 2 s, perfectly synchronized by construction; the reference
  measures 0.0 ms with the same analysis). Playing it fullscreen while the
  engine records and measuring the flash (per-frame `signalstats YAVG` peak)
  against the beep (game stem energy burst) in the saved clip showed the
  audio **258 ms ahead** with the default NVENC HQ h264 config.
  - Cause: the video pipeline (WGC → CFR pacer → encoder lookahead → TS mux)
    lags the audio path by a constant that depends on the encoder.
  - Fix: `av_sync_offset_millis(codec, enc_name)` in `wgc.rs` moves the audio
    window end earlier by the measured amount (no user calibration, no
    slider). Constants measured on this machine at 1080p60/HQ:
    **h264 NVENC 258 ms, hevc NVENC 264 ms, libx264 48 ms**.
  - Verified per codec with the same test: h264 **+7 ms**, x264 **−3 ms**,
    hevc **+25 ms** (imperceptible). Re-measure with
    `live_av_offset_capture` (env `MOONCLIP_AVTEST_CODEC`) after encoder,
    preset or fps changes and update the constants.
  - New live probes (ignored by default): `live_tone_coverage` (continuous
    1 kHz tone on the captured device → game stem covered 100%, 0 gaps),
    `live_tone_after_silence` (8 s of silence then the tone → the loopback
    resumes and captures it), `live_av_offset_capture` (flash+beep). They
    prove the loopback itself is reliable while audio renders; the missing
    desktop-audio heads seen in user clips correspond to periods with **no
    rendering stream** on the captured endpoint (the engine idles and WASAPI
    delivers no packets). The UI signal meters now make that visible live.

- **Windows A/V: keyframe-aligned clip start (2026-09-16)** — the video window
  is cut by bytes (CBR) at arbitrary positions, so it could start mid-GOP: the
  muxer dropped the pre-keyframe packets and the video track started up to
  1.6 s after the audio (measured `start` offsets 1.567 s / 0.517 s / 0.733 s
  on real clips, GOP = 2 s). Players that ignore the MP4 edit list shifted the
  audio against the video by that amount (the visible delay); players that
  honor it showed a black/frozen head. OBS's replay buffer never has this: it
  cuts at a keyframe and aligns audio to the same timestamp.
  - Fix: after staging the cut TS, probe the first decodable frame's PTS with
    the embedded ffmpeg (`-vf showinfo -frames:v 1 -f null -`; the decoder
    skips to the IDR) and mux with `-ss <t+1ms>` on all four inputs (TS + the
    3 WAVs), so the file starts at 0 with video and audio aligned. Fallback
    `t=0` when the probe fails; trim sanitized (0.05–3.5 s, must leave ≥1 s).
  - Live verification: `live_engine_restart` (two sessions — the settings
    change path — with a mid-GOP cut) asserts 3×AAC, video track starts at 0
    and duration 4.5–6.5 s; `live_buffer_and_save` also asserts the video
    start; unit tests cover the `pts_time` parser and trim sanitization.
  - Known caveat: running the whole `live_` suite in ONE process triggers an
    ACCESS_VIOLATION inside `windows-capture`'s `start_free_threaded` on the
    fourth session (multiple tokio runtimes + capture sessions). Every live
    test passes alone and the app's restart path (single runtime) is fine;
    run live tests individually until upstream fixes it (2.0.1 is the latest).

- **Windows audio: click-free assembly + stall recovery + telemetry (2026-09-16)**
  — the "static/ground noise" in saved clips came from the per-block absolute
  placement: WASAPI block timestamps drift a few samples from their sample
  count and every sub-millisecond drift was materialized as silence
  (measured 289 sub-50 ms gaps in a 6 s capture, ~48/s). Stems are now
  assembled by **sample continuity** (`build_stem_window`): gaps ≤ 50 ms are
  fused, only real holes become silence. Live test: `fused=289 gaps=0
  silence=0.0s`.
  - Delivery telemetry per save:
    `stems: window=… game(lag=… lat=… covered=… fused=… gaps=… silence=…)
    mic(…)` — `covered` vs `window` exposes a loopback that stops delivering
    (a real session showed game covered 20.2 s of a 32 s window while mic
    covered 38 s).
  - **Stall watchdog**: a stream with no blocks for > 3 s is reopened once
    per episode (driver stalls raised no error before); when the selection is
    `default_output`/`default_input` the supervisor follows Windows default
    device changes (2 s poll) and re-links automatically.
  - UI signal meters now publish **windowed peaks** every 500 ms (cumulative
    max is still logged), so a stalled capture visibly drops to zero instead
    of showing a stale peak.
  - cpal delivery latency is now measured per stream
    (`callback - capture`): 0 ms loopback / ~10 ms mic on the test machine —
    i.e. the residual A/V offset is NOT delivery latency and needs the
    sync-test measurement (see `09_WINDOWS_HANDOFF.md` §6).
  - Gate: 31 unit tests (new: sub-fuse drift fusion, latency offset,
    window clip/pad), 3 live tests green, `pnpm build` green.

- **Windows audio sync fix (2026-09-16)** — the desktop (game) stem could be
  seconds out of sync: WASAPI loopback delivery is not continuous (idle
  endpoints deliver nothing, device changes kill the stream, drivers can lag)
  and the save path assumed "last sample = now". Measured on real clips: the
  game stem ended 4.8–6.5 s before the clip end while the mic covered the
  full window, so the desktop audio played late and the first track (Mix,
  the one every player uses) inherited the shift.
  - `os/windows/audio.rs` rewritten around **timestamp-anchored rings**: each
    block keeps the QPC capture timestamp cpal reports; the save path
    rebuilds both stems over the same wall-clock window ending at the newest
    capture across streams, inserting silence where a stream has gaps. A
    lagging/gappy loopback can never drag the track out of position again
    (the old sample-count tail also truncated the Mix when one stem was
    shorter).
  - All cpal sample formats supported for loopback/mic (I8/I16/I32/I64/U8/
    U16/U32/U64/F32/F64) — only F32/I16/U16 worked before.
  - Stream errors mark the stream dead and a supervisor thread reopens it on
    the same device (endpoint invalidated / unplugged) keeping ring history;
    `stream_errors()` feeds the save log.
  - Save log now reports sync telemetry: `stems: window=… game_lag=… ms
    mic_lag=… ms captured=(game …s, mic …s)`.
  - Device list offers `default_output`/`default_input` ("system default")
    again on Windows (Linux already had them via GSR); matching is
    case-insensitive; a failed device/codec restart reverts the setting and
    brings the buffer back with the previous value; mux stderr is captured
    (no more `non-existing PPS` console spam).
  - UI: live game/mic signal meters + "no signal from the selected device"
    hint + 3-track note in Settings → Audio (`audio_peaks` command; Linux
    returns null and hides the meters).
  - Gate: 29 unit tests (incl. new window-placement regressions), 3 live
    tests green (6 s live buffer: `game_lag=0 ms`, `mic_lag=23 ms`),
    `pnpm build` green, zero-`cfg` grep empty.

- **Windows trip: toolchain + engine bug pass (2026-09-16)** — fresh machine
  setup plus six parity bugs fixed in the WGC backend; unit-tested and
  live-verified on the RTX 3060:
  - Environment was blocked: VS BuildTools 18 had headers/libs but no compiler
    binaries and no Windows SDK (`cargo check` died with `linker link.exe not
    found`). VCTools workload + Windows SDK 10.0.26100 installed via
    `vs_installer modify`. pnpm 12.4.2 installed (user prefix), `pnpm install`,
    pinned BtbN ffmpeg fetched with `build-aux/fetch-ffmpeg.ps1` (sha256 +
    encoder assert).
  - `pump_frames` rewritten to a wall-clock CFR schedule. WGC delivers at
    monitor refresh (`MinimumUpdateIntervalSettings::Default` does not
    throttle), and every fresh frame used to be written and timestamped as
    `1/fps` — a 60 Hz desktop at fps=30 or a 144 Hz monitor at fps=60 advanced
    the encoded timeline up to ~2.4x and desynced the saved clip. Now fresh
    frames are absorbed (freshest wins) and exactly one frame is written per
    CFR tick; regression test floods 50 frames at 10 fps and asserts ≤1 write.
  - A/V alignment in `save_clip`: the audio snapshot is taken back-to-back
    with the ring cut (was taken after ~200-500 ms of TS staging), the cut TS
    is probed, and `video_window - audio_tail` becomes leading silence
    prepended to each stem (sample-exact, no keyframe seeks). Previously the
    audio window (duration) was muxed at t=0 against a video window of
    duration+2 s, so saved clips played with audio ~2 s ahead of the image.
  - ffmpeg stderr is drained by a background reader into a bounded 200-line
    ring (an unread 64 KB pipe can block the encoder mid-capture — same class
    the Linux GSR backend already fixed). `log_tail()` returns it.
  - `check_alive()` + `log_tail()` implemented: a closed encoder pipe or an
    exited child now marks the engine dead, so the 2 s watchdog and the UI
    poll surface encoder crashes instead of a silently frozen buffer.
    `impl Drop` stops the WGC session on a helper thread (`CaptureControl`
    has no Drop; the watchdog drops dead engines without `stop_buffer()`, so
    the capture thread leaked).
  - Save pipeline serialized (`AppState.save_lock`) and Windows clip names get
    `_2`, `_3`… when the second already has a file: same-second saves used to
    overwrite the previous clip before it was indexed (`mux -y` + per-second
    name), leaving a ghost DB row and losing the clip.
  - Cosmetic: `backend_name()` no longer returns "(stub)" (shown raw by the UI).
  - Gate: `cargo check` + `cargo test` (28 pass, 3 live ignored) green, live
    WGC/NVENC/WASAPI tests pass (1920x1080@60, save 751 ms, 2/2 audio),
    `pnpm build` green, zero-cfg grep empty.
  - Still pending: user in-game F9 pass (see `09_WINDOWS_HANDOFF.md` §4).

- **Embedded local install (2026-09-16)** — `pnpm tauri:build:linux` bundles
  the resolved sidecars via a per-OS resource overlay: prebuilt GSR
  (`gpu-screen-recorder` + `gsr-kms-server`) and a pinned static FFmpeg (BtbN
  `linux64-gpl`, NVENC + libx264) land in
  `/usr/lib/MoonClip/binaries/x86_64-unknown-linux-gnu/`, so the RPM is fully
  self-contained. `build-aux/fetch-ffmpeg.sh` stages the pinned Linux ffmpeg
  (johnvansickle dropped: its static build has no NVENC, needed by
  save-scale). `pnpm app:install` (build-aux/install-local.sh) replaces the
  RPM, re-applies the KMS cap (updates reset file capabilities) and adds a
  hidden appId desktop alias for KDE/Wayland. Packaged builds now apply the
  WebKitGTK Wayland workaround from code (`os::prepare_environment`) instead
  of relying on cargo's dev env — fixes the Gdk `Error 71` crash at first
  paint in the installed app. Settings → Video gained a one-click
  "Fix permissions" row shown when the KMS cap is missing.

- **Configurable clip hotkey (2026-09-16)** — Settings gains a recorder-style
  "Clip hotkey" row: click, press a combo, Esc cancels; "Reset to F9" appears
  when changed. `set_hotkey` canonicalizes/validates via `HotKey::from_str`,
  re-registers through the global-shortcut plugin (restoring the previous
  binding if the new one is taken) and persists in `settings.hotkey`; startup
  now registers the stored value (fallback F9 + console warning when it is
  unavailable). Bare keys are limited to F1–F12/PrintScreen/Pause, anything
  else needs a modifier. Status card refreshes via `onHotkeyChange`; IPC list
  + README updated; unit tests for normalization.

- **Capture robustness + CPU (x264) option (2026-09-14)**
  - Root-caused the mid-game capture stall: GSR's stdout/stderr were piped but
    never read; the 64 KB pipe buffer fills (GSR logs frame telemetry several
    times per second, more under shader-compile/game load) and GSR blocks
    mid-capture. Both pipes are now drained by background readers into a
    bounded 200-line ring; error/warning lines also reach the dev console.
  - Engine liveness: `check_alive()` (`try_wait`) runs on the `engine_status`
    poll and on a 2 s backend watchdog (the webview pauses while tray-hidden,
    so the UI poll alone cannot detect it during gaming); a dead engine is
    dropped, the last GSR error lines surface in the UI, and
    `moonclip://engine-stopped` fires + notification. No auto-restart by
    design.
  - Optional CPU encoding: GSR `--info` `h264_software` maps to the app's
    `x264` codec, spawned as `-k h264 -encoder cpu` (GSR rejects
    `h264_software` as a `-k` value). Save-scale uses `libx264` for x264;
    locale labels/notes updated (ES/EN).
  - Caps fix: `os/linux/caps.rs` checks/sets `cap_sys_admin` on
    `gsr-kms-server` (upstream's actual target; without it GSR launches the
    helper through `pkexec` and prompts for admin on every capture start).
    README + docs 02 corrected.

- **Audio device list + save robustness + hotkey test card removed (2026-09-14)**
  - `list_audio_devices` used `LinuxGsrEngine::resolve_binary`, which only
    checks next-to-exe/PATH, so it failed in dev (sidecar lives in
    `src-tauri/binaries/<triple>/`): Settings showed only
    `default_input`/`default_output` plus a red "gpu-screen-recorder not
    found" line while the engine (via `binary::backend_binary`) recorded
    fine. The command now takes `AppHandle` and uses the same resolver as the
    engine on Linux; Windows accepts and ignores it (cpal, no sidecar). IPC
    wire unchanged.
  - `save_clip` replaced the fixed 400 ms sleep + "newest mp4" (could return
    a stale clip or fail on slow disks / large rings) with a 5 s poll for a
    `.mp4` whose mtime is >= the save signal; hermetic unit test added.
  - Removed the Phase 1 "Global hotkey test" card (pulse counter, last press,
    test notification) plus dead frontend code/locale keys; the backend
    `moonclip://clip-hotkey` event still fires for future consumers.

- **Responsive layout (2026-09-14)** — App usable on narrow/portrait screens
  (user's rotated 768x1360 second monitor; `minWidth` was 900 so the window
  did not fit). `tauri.conf.json` min `420x420`. Sidebar becomes a 56 px icon
  rail below `lg` (tooltips, icon-only start/stop; hotkey card/language hidden
  until `lg`). Shell padding/gap scale, aura clamped to viewport, main
  `p-3/sm:p-4/lg:p-6`. Gallery `ClipRow` and settings/AppManager rows stack
  below `sm` (thumbs `aspect-video` full width), inputs fluid. Topbar wordmark
  from `sm`, tagline from `md`; also fixed the last hardcoded `MoonLit` (was
  split as `Moon<span>Lit</span>` so the rename grep missed it). Docs 07
  updated.

- **Rename MoonClip (2026-09-14)** — Project renamed from MoonLit everywhere:
  identifier `dev.souriscg.moonclip`, crate/package `moonclip`, clips homes
  `~/Videos/MoonClip` / `%LOCALAPPDATA%\MoonClip\Clips`, DB `moonclip.db`,
  keyring service `moonclip`, env overrides `MOONCLIP_GSR_BIN` /
  `MOONCLIP_FFMPEG`, events `moonclip://*`, sidecar dir `moonclip-gsr/`,
  Tailwind/CSS/component names (`moonclip-*`, `MoonClipLogo`,
  `MoonClipStarfield`), dev `.desktop` `dev.souriscg.moonclip`. No data
  migration (no pre-rename installs exist). Historical log entries and commit
  subjects below intentionally keep the old name.

- **Phase 3-win — Windows capture engine (2026-09-07, commits `681529d`→`7078a13`)**
  Native WGC replay engine behind the same `CaptureEngine` surface; Linux
  untouched except a mechanical `list_audio_devices` signature alignment
  (argless on both backends; Linux resolves its GSR binary internally).
  `commands.rs` IPC contracts unchanged (only a native-binary fallback path,
  still zero-`cfg` — the rule grep prints nothing).
  - `video.rs`: DXGI vendor (dGPU by VRAM, Basic Render skipped), WGC
    monitors + `resolve_monitor`, probed `offered_codecs` (live
    micro-encode per candidate, cached; AV1 only where it encodes).
  - `audio.rs`: cpal loopback + mic, 48 kHz stereo stems, live 0–200%
    gains via atomics, mix tap full-fidelity (Linux parity).
  - `wgc.rs`: WGC → ffmpeg (NVENC/AMF/QSV/libx264, CBR ladder, 2 s GOP,
    NVENC HQ only on nvidia+h264/hevc) → MPEG-TS RAM ring; save cuts the
    window and muxes 3×AAC (mix/game/mic) in GSR filename style.
  - `x264` always offered (user decision): ladder follows h264,
    `TranscodeEncoder::X264` → `libx264`, EN+ES locale keys,
    `THIRD_PARTY.md` corrected (shipped ffmpeg builds link libx264; the
    project was already GPL-3.0-only via GSR).
  - OS floor documented (Win10 1903+/Win11) in `09`/`02`/`08`.
  - Verified live on RTX 3060: 1080p60 NVENC buffer, save → h264 + 3×AAC;
    `cargo check` + `cargo test` (15 pass) green on
    `x86_64-pc-windows-msvc`. Full Linux `cargo check` from Windows is
    blocked by missing GTK system libs (environmental); the Linux-side
    diff is mechanical and Linux CI must confirm before release.
  - Still needs a user in-game pass: F9 → clip <1s, audible gain sliders,
    device/monitor/codec switching with restart notice.
- **Phase 3-win batch 2 (2026-09-07, `fbb4286`→`052e08c`): audio stock fix,
  embedded ffmpeg, AV-safe home, zero warnings**
  - Stock-install audio failure (root-caused): settings seed GSR magic ids
    (`default_input/output`) that never match cpal names → both streams
    failed. `find_output/input_device` now resolve them to OS defaults;
    regression live test included.
  - FFmpeg fully embedded: `build-aux/fetch-ffmpeg.ps1` pins BtbN
    `win64-gpl` monthly + sha256 + encoder-set assert; `resolve_ffmpeg`
    via `sidecar.rs` (sidecar-first, PATH dev-only); `CaptureConfig`
    carries the path (Linux ignores); probes use the shipped binary.
    Full e2e re-verified against the pinned binary. No shell plugin
    (line-events only — raw pipes need the bare path via resources).
  - AV-safe home: `%LOCALAPPDATA%\MoonLit\Clips` on Windows (outside
    Controlled Folder Access + OneDrive, no elevation), Linux unchanged;
    one-time boot migration of legacy files, DB rows untouched, custom
    folders never moved.
  - `scale_arg` moved to `os/linux` (GSR-only); NVENC HQ recipe completed
    (`spatial-aq 1`, `multipass disabled`, accepted by Gyan 9 + BtbN).
  - Static CRT (`crt-static`, target-scoped): dumpbin shows system DLLs
    only — no VC++ Redistributable needed.
  - Gate: 17 unit tests pass, 3 live HW tests pass, zero `cargo` warnings,
    zero-`cfg` grep empty, `pnpm build` green, full `cargo build` links.
- **Thumbnail fix (2026-09-07): `ffmpeg thumbnail failed` on every Windows
  save.** NVENC/swscale emit limited-range yuv420p, which ffmpeg 9's mjpeg
  encoder rejects (`THUMB_EXIT=-22`, reproduced). `make_thumbnail` now
  passes `-strict unofficial` (pixels untouched) and takes an adaptive seek
  (`min(1 s, half the probed duration)`) so sub-second clips also index.
  Hermetic regression test (`lavfi→libx264` limited-range fixture, CI-safe);
  verified against Gyan 9 and the pinned BtbN (exit 0, valid `.jpg`).
- **Timeline collapse fix (2026-09-07): ≤2 s clips on 10 s+ runs.** WGC
  delivers only on screen change; the unpaced pump let the encoded timeline
  collapse to the activity burst, and `-shortest` then cut the audio to it
  (telemetry proved it: `wgc_in=123 pump_out=259` over 6 s). Fixes: CFR
  pacer with deadline catch-up in the pump (telemetry now
  `wgc_in=81 pump_out=425`, duration tracks wall-clock), `apad` per stem +
  full-length silence placeholders (short stems can never truncate), MIX
  derived at save from aligned solo tails (live mix-ring interleaved chunks
  — removed), e2e asserts probed duration ∈ [4,8] s for a 6 s run, save
  logs pump telemetry, mic-privacy note in `09`.
- **Slow-save investigation (2026-09-07): F9→ding took 30 s+.** Found the
  dev disk (C:) at **477 MB free** with a 29 GB `target/` — disk pressure
  alone stalls every stage (80 MB TS + WAVs + 75 MB clip + AV scans).
  `cargo clean` reclaimed it (26.5 GB free). Shipped with the same commit:
  per-stage timing logs (`save total/engine/probe+thumb/db`, `mux wav/mux`,
  `wgc-save`) and parallel probe+thumbnail (`join!`, 1 s seek with near-head
  retry for sub-second clips). Next F9 prints exactly where seconds go.

- **Phase 0/1** — Scaffold, tray minimize-to-tray, F9 global shortcut with notification, MoonLit glass layout, pausable canvas starfield, ES/EN i18n. Manual test: F9 counted globally, tray restore OK.
- **Phase 1-fixes** — Frameless window + custom topbar (drag, minimize, maximize, close-to-tray), MoonLit moon+play CSS logo from moonlit.souriscg.dev, time-based soft starfield twinkle, Rust 400ms + frontend 300ms F9 dedupe. Manual test passed by user.
- **License** — Adopted `GPL-3.0-only` (gpu-screen-recorder is GPL-3.0-only per Arch/Alpine/Artix). Verbatim FSF text, metadata in `package.json`/`Cargo.toml`, README section.
- **Phase 2-fixes** — Single-source locale (`useLocale`: DB + i18next in sync, both directions), added missing `dialog:allow-open` capability (folder picker was silently rejected), visible errors on browse/save, logo glow contained (was `z-index:-1` leaking → corner artifact).
- **Maximize flicker (root-caused)** — Tauri natively toggles maximize on double-click over `data-tauri-drag-region` (internal-toggle-maximize, tauri#12006); our own JS dblclick handler double-toggled (flicker). Fix: removed JS dblclick, native owns the gesture; buttons keep atomic `toggleMaximize` + debounce + OS-synced state. Capability `allow-internal-toggle-maximize` added explicitly.
- **Dev env** — `src-tauri/.cargo/config.toml` sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` for all cargo runs (fixes Wayland `Error 71` crash at first paint on Fedora/GNOME); `pnpm tauri:dev` is the official launch command.
- **UI batch** — MoonLit icon set regenerated from `build-aux/moonlit-icon.svg`
  (taskbar + tray via `default_window_icon`); `lang.es/lang.en` translated in
  sidebar + select (last hardcoded Spanish gone); `opener:allow-open-path` +
  `opener:allow-reveal-item-in-dir` added (`opener:default` does not include
  those commands — root cause of the open-video error); disk-space note under
  the resolution selector (ES/EN).
- **Native scaler patch (scheduled, not implemented)** — After Flatpak + MS
  Store ship, before signing `.exe`/`.msi`: raise GSR's downscale filter from
  hardcoded `GL_LINEAR` (bilinear, no mipmaps — proven in pinned source
  `src/window_texture.c`) to Bicubic (Lanczos under test). Same RAM/bitrate/
  latency/save path. Acceptance: stock vs patched 720p on a 1:1 monitor.
- **Authorship rewrite (2026-09-06)** — All 26 commits re-signed from
  `SourisCG <souris@souriscg.dev>` (assumed by the agent at `git init`, never
  confirmed) to `Sebastián García <sebastian.garciab2004@gmail.com>` (user's
  global identity). Content, messages and dates byte-identical
  (`git diff` old vs new tip: empty). Original history preserved in branch
  `main-backup-20260906` (local + remote, keep until told otherwise).
  Old `build <hash>` seals below resolve via the backup branch + map.
- **Wayland taskbar association, dev flow (2026-09-06)** — Window showed the
  generic Wayland icon in the taskbar while the tray icon rendered fine.
  Root cause: on Wayland the taskbar matches by `appId ↔ .desktop`
  (`dev.souriscg.moonlit` via `app.enableGTKAppId`), and no dev `.desktop`
  existed in the repo; a stale `com.souriscg.MoonLit.desktop` (wrong
  identifier/WMClass, `Exec=MoonLit` pointing nowhere) also conflicted.
  Fix: `app.enableGTKAppId: true` in `tauri.conf.json`,
  `build-aux/dev.souriscg.moonlit.desktop.template` +
  `build-aux/install-dev-desktop.sh` (`pnpm desktop:install`) installing a
  validated dev entry + removing the stale one. Tray is unaffected (explicit
  `TrayIconBuilder` pixels). Verified by user on KDE Wayland.
- **Transparent icon set (2026-09-06)** — `src-tauri/icons/` regenerated from
  `build-aux/moonlit-icon.svg` as fully transparent (rounded artwork, no
  opaque backdrop). `tray-icon.png` currently mirrors the set; the dedicated
  tray asset separation is documented as future work in `07_UI_MOONLIT.md`.
- **HEVC save fix (2026-09-06)** — h265 clips failed to save: NVENC HQ opts
  are now per-codec (`nvenc_hq_opts`: `high` for h264, `main` for hevc) with
  unit tests in `video_quality.rs`.
- **Purge missing clips (2026-09-06)** — New `purge_missing_clips` command +
  gallery button: drops DB rows whose files no longer exist on disk.
- **Gain range 0–200% (2026-09-06)** — Track sliders extended from 150 to 200
  (clamps in `os/linux/audio.rs::set_volume`, `read_gains`, `set_track_gain` +
  `TrackMixer` slider). Defaults stay 100/100; 200% is a boost tool for quiet
  sources — safe ceilings documented in `02_CAPTURE_ENGINE.md` (game ≈100, it
  already peaks near 0 dB at unity; mic ≈120–150). PipeWire acceptance of 200%
  verified live; user-verified on a real clip.

## Hash map (old → new, same order/messages/dates)

| Old | New | Subject |
|---|---|---|
| `6965fac` | `ccf9451` | feat(phase1): tray minimize-to-tray + F9 global hotkey + MoonLit glass UI + pausable starfield + i18n es/en |
| `8e4b5c1` | `9a81b9a` | fix(phase1): custom frameless topbar + MoonLit CSS logo + smooth starfield + F9 dedupe |
| `2b99add` | `2f1234e` | chore(license): adopt GPL-3.0-only (required by gpu-screen-recorder sidecar) |
| `c72edba` | `b8f218e` | feat(phase2): rusqlite persistence with relative paths + keyring secrets + settings UI |
| `f8884c6` | `94536d3` | docs: mark phase 2 done in PROGRESS |
| `982890a` | `9f3cc75` | fix(phase2): synced locale source, dialog permission, logo glow containment |
| `1d14c82` | `943d024` | fix(dev): permanent Wayland workaround via .cargo/config env |
| `259dcdb` | `a885c28` | docs: log wayland dev fix in PROGRESS |
| `69fdd4a` | `f051f0c` | fix(phase2): stateless locale from i18n + hardened maximize toggle |
| `3c93467` | `4b14c1e` | debug(phase2): temp maximize flicker logging (to be removed with fix) |
| `4a9d44d` | `75052f0` | fix(phase2): atomic maximize toggle + delayed drag-area maximize |
| `2b2c5e2` | `71848fc` | fix(phase2): let Tauri own drag-area dblclick maximize |
| `502601e` | `bbddf69` | docs: log maximize root cause in PROGRESS |
| `00a70b3` | `ff9e586` | feat(phase3): GSR replay engine + dual audio + record UI (Linux) |
| `d0fec68` | `a44eb46` | feat(phase3): embedded GSR build script + live per-track volumes |
| `94d6d8a` | `f371bba` | feat(phase3): embedded GSR binary + aac tracks |
| `cbb29bd` | `a60d51a` | fix(phase3): stream matching, thumbs, duration restart, slider UX |
| `b0ab0a5` | `4ae01b3` | fix(phase3): exact stream match, thumbs, restart keys, devices, quick-open |
| `efec83a` | `ce54586` | refactor+fix(phase3): OBS-style os/ layout, asset protocol, real durations |
| `2e06904` | `2f075da` | fix(phase3): gallery state+errors, stream debug log, responsive pass |
| `0c6dd61` | `949eaae` | fix(phase3): icon-only gallery, self-clearing errors, build stamp |
| `a9e5377` | `7a2ca1b` | feat(phase3): 3-track layout, mix first (plays everywhere) |
| `e95189c` | `3229e91` | fix(phase3): reveal fallback to openPath(parent) |
| `4f30c08` | `1981222` | fix(phase3): camelCase revert, scroll, title, resize grips |
| `328e911` | `eae081d` | fix(phase3): clip filename as row title, probe cleanup |
| `f290be2` | `a098988` | feat(phase3): Medal bitrate ladder + NVENC HQ recipe + video settings |

## V3 — embedded isolated OBS engine (2026-09-18)

- **Capture engine replaced end-to-end on BOTH OSes** by one shared engine
  (src-tauri/src/os/obs.rs): MoonClip now ships an embedded, isolated OBS
  Studio and drives it with the bundled obs-cmd over a private
  obs-websocket. GSR (os/linux/gsr.rs + uild-aux/build-gsr.sh) and the
  ffmpeg gfxcapture engine (os/windows/{engine,ring,pts,dsp,mux,encode,
  detector,audio,caps}.rs) are deleted.
- **Isolation ("que no se vean ni en pintura")**: OBS is launched with
  --config-dir <MoonClip dir> (Windows `%LOCALAPPDATA%\MoonClip\obs`,
  Linux `~/.config/MoonClip/obs`), `--multi`, generated profile/collection
  `MoonClip`; the user's OBS config is never read/written. Hard guards:
  `--config-dir` support probe + config-directory guard, private websocket
  port + generated password, Repair button that only deletes OUR config root.
- **Anti-cheat (~0 hook risk)**: generated scenes use ONLY compositor sources
  (monitor_capture DXGI / window_capture WGC on Windows, PipeWire portal on
  Linux). `game_capture` (process hooking) is forbidden and test-enforced.
- **Audio**: 3-track layout preserved (Mix 320k / Game 320k / Mic 192k, Mix
  first) via profile `RecTracks=7` + source mixers; live mutes through
  `obs-cmd audio`; gains persisted into the scene (restart-once notice);
  `audio_peaks` is null (no fake meters).
- **New Settings UI** (VideoSection.tsx): Medal-style Moon preset cards
  (Low/Medium/New Moon/First Quarter/Waning Gibbous/Full Moon) with Medal
  recommended ranges, FPS 24-144, codec H264/H265/AV1, GPU/CPU encoder,
  custom bitrate/FPS, duration, container, RAM/performance warnings, and an
  "Motor OBS (aislado)" panel (ObsEngineSection.tsx).
- **Optional first-run hardware test** (	est_hardware + wizard): start with
  candidate values (not persisted), record ~10 s, save, validate duration/size,
  suggest one step down on failure, restore the previous buffer state.
- **Packaging**: uild-aux/fetch-obs.ps1 pins OBS portable
  32.2.2 (sha256 verified) + obs-cmd 1.0.2 (sha256 verified, the release
  that fixed replay-save flushing); etch-obs.sh does the Linux .deb
  best-effort unpack plus a system-obs fallback; Tauri overlays bundle both.
- **Gates**: cargo test 36 passed, cargo clippy --all-targets -- -D
  warnings clean, pnpm build clean, zero-cfg grep clean, game_capture
  only in guard constants/tests. Pending (owner): Windows in-game pass,
  system-OBS coexistence, Linux portal run.

### V3.1 — aislamiento corregido: modo portable real (2026-09-18)

- **Hallazgo (probado en vivo)**: OBS Studio 32.2.2 en Windows IGNORA
  `--config-dir`: arrancó en `Portable mode: false` y escribió en
  `%APPDATA%\obs-studio` (log + user.ini), descartando ese enfoque.
- **Fix**: MoonClip ahora **copia** el OBS embebido a una carpeta escribible
  propia (`%LOCALAPPDATA%\MoonClip\obs` en Windows,
  `~/.local/share/MoonClip/obs` en Linux), escribe `portable_mode.txt` y
  arranca con `--portable`. OBS escribe `config/` dentro de esa copia por
  construcción; la config del usuario queda intocable. Marker de build para
  re-copiar cuando cambie la versión embebida; el `Repair` sigue borrando
  solo nuestra config.
- **Linux system-obs**: sin copia posible → `--config-dir` + config guard.
- **fetch-obs**: ya no ejecuta OBS jamás (ni `--help` ni `--version`) y
  quedó pinneado y verificado por sha256: OBS 32.2.2 + obs-cmd v1.0.2.
- Gates: `cargo clippy --all-targets -D warnings` limpio, 37 tests,
  `pnpm build` limpio.

### V3.2 - Custom = todas las opciones de video de OBS + volumen en vivo (2026-09-19)

- **Menu simplificado**: la vista simple deja solo monitor + presets Moon +
  duracion + contenedor. El toggle Custom abre TODAS las opciones de salida
  de video de OBS: picker de encoder (catalogo pinneado por plataforma x
  vendor detectado x probe ffmpeg), schema completo por familia
  (NVENC/x264/QSV/AMF/VAAPI) + pestana Video (resolucion, filtro, FPS
  comun/entero/fraccionario, color). Cada opcion tiene estado Auto (= default
  de OBS, clave omitida) y boton "Restaurar automatico".
- **Registry `os/encoder_options.rs`**: unica fuente de verdad, tablas
  extraidas de obs-studio@32.2.2 (nvenc-properties.c, obs-x264.c,
  obs-qsv11.c, texture-amf.cpp + strings de la DLL, obs-ffmpeg-vaapi.c).
  Hallazgo: nuestras claves NVENC eran de OBS 29 (`preset2`/`psycho_aq`/
  `gpu`, ignoradas en silencio por OBS 32) -> ahora `preset`/
  `adaptive_quantization`/`device`; QSV usaba `preset`/`async_depth`
  inexistentes -> `target_usage` TU1-TU7 + `latency`.
- **Auto por familia (deteccion de GPU automatica)**: vendor por DXGI;
  NVENC/x264 recetas medidas; AMF/QSV/VAAPI receta Auto (CBR + bitrate, OBS
  decide el resto) al no haber hardware para validar. Auto tambien corrige
  el mapeo Linux AMD (cada codec a su id VAAPI: antes HEVC iba al id H.264).
- **Persistencia**: migracion 008 (`custom_encoder_json`,
  `custom_video_json`, validados pre-escritura); fix whitelist db.rs
  (`video_encoder`/`gpu_index` antes rechazados -> el toggle GPU/CPU fallaba).
- **Prueba real**: `test_hardware` acepta payloads Custom sin persistir
  (boton "Probar 10 s") y verifica el stream real del clip
  (`ffmpeg -i` parse: codec, alto y fps deben coincidir).
- **Volumen en vivo**: `set_track_gain` aplica via
  `obs-cmd input volume --set` sin reiniciar (fallback a un reinicio si el
  live falla); TrackMixer muestra errores en vez de tragarlos.
- Gates: `cargo test` 61 passed, `cargo clippy --all-targets -- -D warnings`
  limpio, `pnpm build` limpio, zero-cfg (solo #[cfg(test)]). Pendiente
  (owner): Probar 10 s en NVENC CBR/CQP + x264 CRF, presets AMD/QSV/VAAPI en
  hardware real, pass in-game.

### V3.3 - compatibilidad por codec + OBS invisible (2026-09-19)

- **Registry por codec**: `OptionSpec` gana `codecs`, `codec_values`,
  `codec_range`, `codec_int_values`, `codec_defaults`, `p010_values`,
  `p010_ints` + reglas de visibilidad multiples (`when`+`and_when`).
  `resolved_options(family, codec)` es la unica fuente para UI, sanitize y
  `validate_pair` (incluye gate 10-bit <-> P010).
- **Correcciones contra fuentes OBS 32.2.2**: AMF usaba claves inexistentes
  (`vbaq`/`enforce_hrd`/`params`/`preanalysis` -> solo existen
  `pre_analysis`/`bf`/`ffmpeg_opts`; `bf` AVC+AV1 0-5; perfil HEVC no existe;
  preset AV1 incluye `highQuality`; rate_control suma HQVBR/HQCBR).
  NVENC `max_bitrate` tambien en CQVBR; x264 `crf` en VBR, `bitrate` en ABR,
  `buffer_size` requiere `use_bufsize`; VAAPI `qp`/`bitrate` en QVBR.
- **UI Custom**: schema por encoder (no por familia), opciones invalidas en
  gris deshabilitadas, `sanitizeVals` descarta al cambiar, bitrate sembrado
  del ladder al entrar/cambiar/resetear, `resetAuto` resetea TODO (encoder
  auto + Video tab), resumen "Aplicado".
- **NumberField** compartido (draft al editar, commit en blur/Enter, clamp):
  migados out_w/h, fps int/num/den, ints del registry, duracion custom,
  max_gb.
- **OBS invisible (Windows)**: `user.ini` sin bandeja, sin
  `--minimize-to-tray`, watcher que oculta la ventana (PID+imagen, titulo
  "OBS ", dialogos intactos), AMUI compartida, staged exe renombrado a
  `moonclip-obs.exe`. Linux: best-effort xdotool/wmctrl.
- **db.rs**: migracion 008 conectada (SCHEMA_VERSION 8).
- Gates: `cargo test` 64 passed, `cargo clippy --all-targets -- -D warnings`
  limpio, `pnpm build` limpio, zero-cfg (solo #[cfg(test)]). Pendiente
  (owner): Probar 10 s, in-game, coexistencia sin bandeja/ventana visible,
  Task Manager agrupado, Linux.
