# 02 — Capture Engine (embedded OBS + obs-websocket)

## 1. Concept

A clip app is a rolling RAM replay buffer, not a recorder. MoonClip V3 owns no
encoder of its own: it ships an **isolated OBS Studio** and controls
it in-process over obs-websocket, driving the OBS replay buffer:

```
Settings (SQLite) -> Rust writes basic.ini + MoonClip scene collection
                     (Wayland: portal RestoreToken pre-seeded if known)
                  -> launches the embedded OBS, config fully isolated
                     (Windows: portable copy; Linux: XDG_CONFIG_HOME)
                  -> obs-websocket replay start (buffer rolls in OBS RAM)
F9                -> obs-websocket replay save (poll last-replay for the new path)
                  -> thumbnail + duration probe + SQLite index (unchanged)
```

No re-encode happens on save: OBS writes the replay file directly (its output
resolution is the resolution the user picked; scaling is GPU-side inside OBS).

## 2. Isolation (the user's OBS is never touched, on either OS)

- **Own config dir:** Windows stages a writable portable copy under
  `%LOCALAPPDATA%\MoonClip\obs` (with `portable_mode.txt`, re-staged by a build
  marker) and launches it with `--portable`. Linux is relocatable
  (RUNPATH `$ORIGIN/../lib64`) and is launched with
  `XDG_CONFIG_HOME=~/.local/share/MoonClip/obs/config`, so OBS writes its
  whole `obs-studio/` tree (config, logs, plugin config) there. Either way,
  `%APPDATA%\obs-studio` / `~/.config/obs-studio` are never read or written
  (verified live on both OSes).
- **Own identity:** generated profile `MoonClip`, collection `MoonClip`, scene
  `MoonClip Capture`, audio sources `MoonClip Game Audio` / `MoonClip Mic`.
- **Coexistence:** `--multi` lets our OBS run next to the user's OBS.
- **Private websocket:** `127.0.0.1:<obs_ws_port>` (default 4456, distinct
  from OBS's 4455) with a generated password persisted in SQLite
  (`obs_ws_password`); obs-websocket is configured inside OUR config dir only.
- **Guards:** `os/obs.rs` kills the child if it does not create
  `<config_root>/obs-studio` within 12 s (i.e. the config redirect was not
  honored). On Linux a system `obs` fallback (dev) uses the exact same
  `XDG_CONFIG_HOME` treatment.
- **Orphans:** force-killed sessions leave OBS children; `kill_orphans()`
  sweeps processes whose image path is OUR bundled binary and whose parent is
  gone. The user's own OBS path never matches.
- **Invisible engine (Windows):** the embedded copy runs with the tray icon
  disabled (`user.ini` `[BasicWindow] SysTrayEnabled=false`, no
  `--minimize-to-tray`) and MoonClip hides its main window right after spawn
  (`conceal_window`: 15 s watcher, PID + staged-image guard, off-screen
  parking). No tray icon, no taskbar button, no window: the instance nests
  under MoonClip as a background child instead of a second "OBS Studio"
  app, and can never be confused with (or touch) the user's own OBS.
  Error dialogs keep their own titles and stay visible for diagnosis.
- **Invisible engine (Linux):** on KDE Plasma, `conceal_window` loads a
  temporary KWin script (DBus `/Scripting`) that matches our exact child PID
  and sets `skipTaskbar/skipPager/noBorder/opacity=0` (the user's OBS never
  matches); the tray icon is disabled in that case. Other desktops keep
  `--minimize-to-tray` + the OBS tray icon as fallback (Wayland has no global
  window control).
- **Repair:** `repair_obs_config` stops the buffer and deletes ONLY the
  MoonClip-owned config root (regenerated on next start).

## 2b. Wayland portal (Medal-style, picker exactly once)

- OBS's PipeWire source requests `persist_mode=2` and stores the returned
  single-use `RestoreToken` in the source settings on every successful Start
  (verified in OBS `screencast-portal.c`). MoonClip pre-seeds that token into
  the generated collection from the `obs_restore_token` setting, and reads the
  refreshed token back after each start with
  `GetInputSettings` over obs-websocket, persisting the newest one.
  Result: the system picker appears **once**; later starts restore silently.
- **First run:** the SetupWizard test step shows the picker (hint text). The
  start path waits for a real stream (screenshot probe) before reporting
  success; if the dialog is cancelled, the start fails loudly and the engine
  stops instead of recording black.
- **Change screen** (Settings → Video → "Cambiar pantalla…") clears the token
  and learned canvas (`clear_portal_token`) and restarts the buffer, so the
  picker appears exactly once more.
- **Canvas learning:** the portal decides the captured size, not our profile.
  After start, MoonClip saves a source screenshot (`SaveSourceScreenshot`),
  parses the PNG size and calls `SetVideoSettings` when the canvas
  differs; the learned size is persisted (`obs_source_width/height`) and used
  as the base resolution on the next start (fixes the old 1080p fallback and
  rotated/portrait monitors).

## 3. Anti-cheat (hard rule, ~0 hook risk)

Kernel anti-cheats (Vanguard, VAC, FACEIT, EAC, BattlEye) block or flag
**hook-based** capture because it injects a DLL into the game process. The
generated scene NEVER uses OBS's `game_capture`; only compositor-level sources:

| OS | Source id | Method |
|---|---|---|
| Windows | `monitor_capture` | WGC display capture (`method: 2`, `force_sdr`, cursor) |
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
- **Mute** applies live through `SetInputMute` (obs-websocket).
- **Gain** applies live through `SetInputVolume` (0-200 % → multiplier); a
  failed live apply falls back to a single buffer restart.
- **Peaks/meters:** obs-websocket exposes `InputVolumeMeters` events, not
  subscribed yet (obws `events` feature) → `audio_peaks` returns `null` (the
  UI hides the meters; no fake data).

## 6. obs-websocket contract

`os/obsws.rs` wraps the `obws` client against OUR private websocket
(`127.0.0.1:<obs_ws_port>`, generated password) and exposes:
- `replay_buffer().start()/stop()/status()`
- `replay_buffer().save()` + `last_replay()` polling until the new path
  appears (the flush-race fix obs-cmd carried for issue #103, now in-process;
  45 s timeout)
- `scenes().current_program_scene()` (scene-live proof)
- `sources().active()` + `save_screenshot()` (portal stream proof + canvas
  learning)
- `config().set_video_settings()` (canvas/output resize)
- `inputs().settings()/set_volume()/set_muted()` (portal token, live gains)

All calls are timeout-bounded; failures surface the error text. Save still
validates the path exists on disk before indexing it.

> The previous `obs-cmd` CLI was removed: its latest release (v1.0.2) ships
> `input settings`, `input volume` and `input mute` as stubs that only print
> "experimental" and never talk to OBS.

## 7. Quality ladder and custom mode

- Ladder (Medal table, CBR): 360p 3M · 480p 5M · 720p 10/7/7M · 1080p
  20/12/8M · 1440p 25/20/15M · 2160p 60/35/25M (h264/hevc/av1).
  `video_quality.rs` exposes `bitrate_kbps`, `recommended_kbps` (Medal
  recommended ranges shown in the UI) and `ring_mb`.
- Ladder encoder recipe comes from the registry (`os/encoder_options.rs`):
  measured NVENC/x264 recipes on validated families, Auto (CBR + bitrate
  only, OBS decides the rest) on AMD/QSV/VAAPI. GPU vendor auto-detects via
  DXGI; unsupported combos fail loudly at start with the OBS log tail.
- Custom mode (video_mode=custom): every OBS video-output option —
  encoder picker (pinned per-platform catalog × vendor × live probe),
  full per-encoder schema (rate control, bitrate, keyframe, preset, tuning,
  B-frames, GPU, free-text opts…), plus the Video tab (output resolution,
  scale filter, FPS common/integer/fractional, color format/space/range).
  Persisted as validated `custom_encoder_json` / `custom_video_json`
  (migration 008); unknown ids/keys/values are rejected before anything is
  written. Each option has an Auto state (omitted key = OBS default).
- Codec compatibility is dynamic: the registry scopes every option/value
  per codec (from OBS 32.2.2 plugin sources); the UI greys out whatever is
  invalid for the current codec, discards it on codec change, and blocks
  10-bit profiles unless the color format is P010. Un-Autoing an option
  seeds the codec-scoped OBS default. Entering Custom seeds the ladder
  bitrate so the first Apply records at the expected rate.

## 8. Save path

1. `save_clip` → obs-websocket `replay_buffer().save()`, then poll
   `last_replay()` until the new path appears (45 s), verify it exists.
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

Since the Custom panel, `test_hardware` also accepts full Custom payloads
(`encoderId`, `encoderSettings`, `customVideo`, NOT persisted) for the
"Probar 10 s" button, and — duration aside — reads the real encoded stream
back from the file (`ffmpeg -i` parse, no ffprobe in the sidecar):
`check_probe` requires codec, output height and fps to match the request,
or the test fails. This is the proof that OBS really applied the settings.

## 10. Live mixer (volume without restart)

`set_track_gain` persists the 0-200 % gain and applies it live through
`SetInputVolume` (obs-websocket) while the buffer runs (trait
`CaptureEngine::set_volume`); only a failed live apply falls back to the
single-restart path. The generated scene still carries the persisted gain,
so the next start renders it even without a running buffer. Mixer errors
are shown in the UI, never swallowed.

## 11. Rust trait (frozen interface)

```rust
pub trait CaptureEngine: Send + Sync {
    async fn start_buffer(&mut self, config: CaptureConfig) -> Result<(), String>;
    async fn save_clip(&mut self) -> Result<PathBuf, String>;
    async fn stop_buffer(&mut self) -> Result<(), String>;
    fn tracks_linked(&self) -> usize;
    fn check_alive(&mut self) -> bool;
    fn log_tail(&self) -> Vec<String>;
    async fn set_mute(&mut self, track: &str, muted: bool) -> Result<(), String>;
    async fn set_volume(&mut self, track: &str, percent: u32) -> Result<(), String>;
}
```

`os/obs.rs` holds the shared implementation (`ObsEngine`) and the
`ObsPlatform` seam; `os/windows/obs.rs` / `os/linux/obs.rs` provide the
platform bits. Selection happens only in `os/mod.rs` (zero-`cfg` elsewhere).

## 12. Acceptance (V3)

- [ ] F9 in-game writes an OBS replay `.mp4` with 3 audio tracks (Mix first).
- [ ] The user's own OBS can run simultaneously; its config never changes.
- [ ] Changing preset/custom settings/monitor/audio restarts the buffer once (notice); mixer gain applies live.
- [ ] `game_capture` appears in no generated file (test-enforced).
- [ ] Hardware test reports a valid clip at the suggested preset; Custom "Probar 10 s" proves codec/resolution/fps.
- [ ] `cargo test`, `cargo clippy -D warnings`, `pnpm build`, zero-`cfg` grep.
- [ ] Windows: BO7/CS2/Vanguard in-game pass (owner).
- [ ] Linux: portal picker once, then clip with 3 tracks; restart shows no picker; "Change screen" re-prompts once; OBS hidden on KDE (owner).
