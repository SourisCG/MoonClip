# 02 — Capture Engine (embedded OBS + obs-cmd)

## 1. Concept

A clip app is a rolling RAM replay buffer, not a recorder. MoonClip V3 owns no
encoder of its own: it ships an **isolated OBS Studio** and a bundle of
`obs-cmd`, and drives the OBS replay buffer:

```
Settings (SQLite) -> Rust writes basic.ini + MoonClip scene collection
                  -> stages a writable PORTABLE copy of the embedded OBS
                     (portable_mode.txt) and launches it
                  -> obs-cmd replay start      (buffer rolls in OBS RAM)
F9                -> obs-cmd replay save       ("Saved replay: <path>")
                  -> thumbnail + duration probe + SQLite index (unchanged)
```

No re-encode happens on save: OBS writes the replay file directly (its output
resolution is the resolution the user picked; scaling is GPU-side inside OBS).

## 2. Isolation (the user's OBS is never touched, on either OS)

- **Own config dir:** Windows `%LOCALAPPDATA%\MoonClip\obs`; Linux
  `~/.local/share/MoonClip/obs`. The embedded OBS is staged there as a
  writable portable copy with a `portable_mode.txt`, so OBS itself writes
  `<copy>/config/obs-studio/...` — `%APPDATA%\obs-studio` /
  `~/.config/obs-studio` are never read or written (this replaces the earlier
  `--config-dir` attempt, which OBS on Windows ignored). A marker file
  re-stages the copy when the bundled build changes.
- **Own identity:** generated profile `MoonClip`, collection `MoonClip`, scene
  `MoonClip Capture`, audio sources `MoonClip Game Audio` / `MoonClip Mic`.
- **Coexistence:** `--multi` lets our OBS run next to the user's OBS.
- **Private websocket:** `127.0.0.1:<obs_ws_port>` (default 4456) with a
  generated password persisted in SQLite (`obs_ws_password`); obs-websocket is
  configured inside OUR config dir only.
- **Guards:** `os/obs.rs` kills the child if it does not create
  `<config_root>/obs-studio` within 12 s (i.e. portable mode was not honored);
  on Linux a system `obs` fallback uses `--config-dir` with the same guard.
- **Orphans:** force-killed sessions leave OBS children; `kill_orphans()`
  sweeps processes whose image path is OUR bundled binary and whose parent is
  gone. The user's own OBS path never matches.
- **Repair:** `repair_obs_config` stops the buffer and deletes ONLY the
  MoonClip-owned config root (regenerated on next start).

## 3. Anti-cheat (hard rule, ~0 hook risk)

Kernel anti-cheats (Vanguard, VAC, FACEIT, EAC, BattlEye) block or flag
**hook-based** capture because it injects a DLL into the game process. The
generated scene NEVER uses OBS's `game_capture`; only compositor-level sources:

| OS | Source id | Method |
|---|---|---|
| Windows | `monitor_capture` | DXGI Desktop Duplication (index-based, `method: 1`) |
| Windows (opt-in) | `window_capture` | WGC window capture (`method: 2`) |
| Linux | `pipewire-desktop-capture-source` | XDG portal (Wayland/X11) |
| Linux (opt-in) | `pipewire-window-capture-source` | XDG portal window picker |

Audio is loopback/PipeWire (`wasapi_output_capture` + `wasapi_input_capture` /
`pulse_output_capture` + `pulse_input_capture`) — no in-game hooking. Tests
assert the forbidden source id never appears in any generated artifact
(`os/obs.rs::collection_never_uses_game_capture` +
`os/windows/obs.rs::video_source_never_game_capture`).

Trade-off (documented): Display capture films the whole monitor (overlays
included). That is the same OS-level path Xbox Game Bar uses; the residual
risk is the same as any overlay/capture tool, not the injection risk.

## 4. Generated profile (basic.ini) — MoonClip-owned

Advanced output + replay buffer, 3 AAC tracks (Mix first), Media ladder CBR:

- `[Output] Mode=Advanced`
- `[AdvOut] RecType=Standard`, `RecFormat2=mp4|mkv`, `RecFilePath=<clips dir>`,
  `RecEncoder=<OBS encoder id>`, `RecRB=true`, `RecRBTime=<seconds>`,
  `RecRBSize=<estimated MB + 50% + 64>`, `RecTracks=7` (`1` when
  compatibility mode is on), `RecAudioEncoder=ffmpeg_aac`,
  `Track1Bitrate=320`, `Track2Bitrate=320`, `Track3Bitrate=192`,
  `Track1Name=Master Mix [Game+Voice]`, `Track2Name=Game/Desktop`,
  `Track3Name=Mic`, `ApplyServiceSettings=false`
- `[Video] BaseCX/CY` = monitor size, `OutputCX/CY` = requested resolution
  (aspect-kept, even), `FPSType=0`, `FPSCommon=24|30|60|120|144`,
  `ScaleType=bicubic`, `ColorFormat=NV12`, `ColorSpace=709`,
  `ColorRange=Partial`
- `[Audio] SampleRate=48000`, `ChannelSetup=Stereo`

`recordEncoder.json` is CBR everywhere: NVENC `preset2=p5 + tune=hq +
multipass=disabled + bf=2 + psycho_aq=1 + no lookahead`; AMF `quality + vbaq`;
QSV `medium + async_depth=4`; x264 `veryfast`; VAAPI (Linux AMD) via
`ffmpeg_vaapi` + `/dev/dri/renderD128`. Encoder ids are resolved per
vendor/codec/platform in `os/{windows,linux}/obs.rs::encoder_id`.

## 5. Generated collection — 3 audio tracks

`sources`: the scene + one display source. `AuxAudioDevice1` = game/desktop
(mixers `1|2`), `AuxAudioDevice2` = mic (mixers `1|4`); in compatibility mode
both are `1` (Mix only). `volume` carries the persisted gain (0-200 % →
0.0-2.0 multiplier) and `muted` the persisted mute, so the saved file layout
stays: Track 1 `Master Mix [Game+Voice]`, Track 2 `Game/Desktop`,
Track 3 `Microphone`.

Live behaviour:
- **Mute** applies in live through `obs-cmd audio mute|unmute <source>`.
- **Gain** lives in the scene: changing it restarts the buffer once with a
  visible notice (`set_track_gain` → `restart_if_running`).
- **Peaks/meters:** not exposed by obs-cmd → `audio_peaks` returns `null`
  (the UI hides the meters; no fake data).

## 6. obs-cmd contract

`os/obs.rs::Obscmd` builds `obs-cmd --websocket obsws://127.0.0.1:<port>/<pw>`
and parses:
- `replay status` → `Replay Buffer is running` / `... is not running`
- `replay save` → `Saved replay: <path>` (v1.0.2 polls until flush); fallback
  `replay last-replay` → `Last replay path: <path>`
- `audio mute|unmute <source>` → exit status only

All calls are timeout-bounded; failures surface the last output line. Save
still validates the path exists on disk before indexing it.

## 7. Quality ladder and custom mode

- Ladder (Medal table, CBR): 360p 3M · 480p 5M · 720p 10/7/7M · 1080p
  20/12/8M · 1440p 25/20/15M · 2160p 60/35/25M (h264/hevc/av1).
  `video_quality.rs` exposes `bitrate_kbps`, `recommended_kbps` (Medal
  recommended ranges shown in the UI) and `ring_mb`.
- Custom mode: FPS ∈ {24,30,60,120,144}, bitrate 3 000–100 000 kbps; invalid
  values fall back to the ladder (`custom_capture_params`, unit tested).
- Encoder preference: GPU or CPU (x264). GPU resolves per vendor; unsupported
  combos fail loudly at start with the OBS log tail, never silently.

## 8. Save path

1. `save_clip` → `obs-cmd replay save` (45 s timeout), parse the path, verify
   it exists.
2. Same-second dedupe renames to `stem_2.mp4`, `stem_3.mp4`… (existing).
3. Duration probe + thumbnail in parallel via the bundled FFmpeg.
4. Insert into SQLite (relative name), ding, `moonclip://clip-saved`.
5. Global hotkey path: F9 → `handle_hotkey` → `do_save_clip` (debounced by
   `HOTKEY_DEBOUNCE_MS`, serialized by `AppState::save_lock`).

## 9. Hardware test (first-run wizard, optional)

`test_hardware(height?, fps?, seconds=10)` starts the buffer with candidate
values (NOT persisted), waits, saves, and validates:
`validate_test_clip` requires real bytes (≥64 KB), a measurable duration
within [50 %, 150 %+2 s] of the requested window. On failure it suggests one
step down (2160/1440 → 720@60, >30 fps → 30) and the wizard offers a retry.
The previous buffer state is restored afterwards; the test clip is deleted.

## 10. Rust trait (frozen interface)

```rust
pub trait CaptureEngine: Send + Sync {
    async fn start_buffer(&mut self, config: CaptureConfig) -> Result<(), String>;
    async fn save_clip(&mut self) -> Result<PathBuf, String>;
    async fn stop_buffer(&mut self) -> Result<(), String>;
    fn tracks_linked(&self) -> usize;
    fn check_alive(&mut self) -> bool;
    fn log_tail(&self) -> Vec<String>;
    async fn set_mute(&mut self, track: &str, muted: bool) -> Result<(), String>;
}
```

`os/obs.rs` holds the shared implementation (`ObsEngine`) and the
`ObsPlatform` seam; `os/windows/obs.rs` / `os/linux/obs.rs` provide the
platform bits. Selection happens only in `os/mod.rs` (zero-`cfg` elsewhere).

## 11. Acceptance (V3)

- [ ] F9 in-game writes an OBS replay `.mp4` with 3 audio tracks (Mix first).
- [ ] The user's own OBS can run simultaneously; its config never changes.
- [ ] Changing preset/encoder/monitor/audio restarts the buffer once (notice).
- [ ] `game_capture` appears in no generated file (test-enforced).
- [ ] Hardware test reports a valid clip at the suggested preset.
- [ ] `cargo test`, `cargo clippy -D warnings`, `pnpm build`, zero-`cfg` grep.
- [ ] Windows: BO7/CS2/Vanguard in-game pass (owner).
- [ ] Linux: portal picker once, then clip with 3 tracks (owner).
