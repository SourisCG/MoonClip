# 11 — Audio subsystem (capture, mixer, meters, monitor)

> Both platforms drive the same embedded OBS engine, so everything in this
> document (gains, mutes, meters, monitor) behaves identically on Linux and
> Windows. Platform code only resolves device ids.

## 1. Sources and saved tracks

The generated OBS collection contains two global audio inputs:

| Source | OBS | Device setting | Mixer tracks |
|---|---|---|---|
| `MoonClip Game Audio` | `AuxAudioDevice1` | `desktop_device` (`default_output` = OS default) | 1 + 2 |
| `MoonClip Mic` | `AuxAudioDevice2` | `mic_device` (`default_input` = OS default) | 1 + 4 |

Saved MP4/MKV layout (compatibility mode `audio_single_track` collapses to one):

1. `Master Mix [Game+Voice]` — AAC 320k, first and only enabled/default track.
2. `Game/Desktop` — AAC 320k, disabled by default.
3. `Microphone` — AAC 192k, disabled by default.

All audio tracks carry `alternate_group = 1` (patched into `tkhd` after the mux,
since ffmpeg cannot write it from the CLI).

## 2. Gains — 0–100, 100 = maximum

- Range is **0–100** everywhere (`read_gains`, `set_track_gain`,
  `gain_to_volume`, `set_input_volume`); 100 is unity (0 dB) and the maximum.
  Legacy persisted values >100 are clamped on read and on scene render.
- The persisted setting (`gain_game`/`gain_mic`) is written into the generated
  collection as the OBS volume multiplier, so a fresh start renders it directly.
- **Live apply** while the buffer runs: `set_track_gain` → `CaptureEngine::set_volume`
  → `obws` `SetInputVolume` (`Volume::Mul(percent_to_mul(pct))`).
- A failed live apply is logged (`[moonclip] audio: …`), reported to the UI
  (`EngineStatus.audio_error`, shown in the mixer) and persisted anyway; the
  value lands on the next start. **A gain change never restarts the buffer.**

## 3. Mutes

`set_track_mute` persists `mute_game`/`mute_mic` and applies live through
`SetInputMute`. Failures are logged and surfaced like gains.

## 4. Live meters (OBS-style, dB scale)

- On every engine connect the app asks obs-websocket for the high-volume meter
  events (`reidentify(EventSubscription::ALL | INPUT_VOLUME_METERS)`) and pumps
  the stream into `os/shared/audio_meters.rs`.
- `InputVolumeMeters` arrives every ~50 ms with `inputLevelsMul` per channel.
  The parser takes the loudest channel's **level** (current) and keeps the
  **peak** separate — painting the peak as level made silent mics read 0 dB.
- Peaks hold for `PEAK_HOLD` (700 ms) and then slide down over `DECAY` (320 ms);
  levels decay to zero when events stop (muted/stopped source).
- `audio_peaks` returns `{ game, game_peak, mic, mic_peak }` (linear
  amplitudes), or `null` when the engine is not running.
- The UI maps amplitudes on the OBS **-60 → 0 dB** scale
  (`src/lib/audio.ts::levelToPercent`): 0 % at -60 dB, 100 % at 0 dB. The meter
  shows fixed zones — green to -18 dB (70 %), amber to -6 dB (90 %), red above —
  plus a white peak tick (`src/components/ui/Meter.tsx`).

## 5. Audio monitor (meters without recording)

- `start_audio_monitor` starts the engine with an **audio-only collection**: no
  capture inputs are rendered (so no screen source and no portal picker) and no
  replay buffer is started. `stop_audio_monitor` stops it.
- `EngineStatus.monitoring` tells the UI; `running` stays `false`.
- Settings → Audio exposes a **Monitor** button (disabled while the buffer is
  running, since meters are already live then) and stops the monitor
  automatically when the section unmounts. The engine is concealed like any
  other run.

## 6. UI

- `src/components/settings/TrackMixer.tsx`: two rows (Game / Mic) with mute,
  horizontal 0–100 fader (styled `input[type=range].fader` in `index.css`),
  percentage and dB readout, live meter with scale labels, and a no-signal hint.
- Fader changes commit live with a **140 ms throttle** (`COMMIT_THROTTLE_MS`);
  the meter polls `audio_peaks` every 100 ms while running/monitoring.
- Both controls are custom-drawn (`appearance: none`) so WebKitGTK and WebView2
  render identical sliders and meters.

## 7. Logging

Every gain/mute apply logs to the engine activity ring and the console, e.g.:

```
[moonclip] audio: game gain 80% applied live
[moonclip] audio: mic mute failed: set mute failed: …
```

The engine ring is visible in Settings → Engine ("Motor OBS"); failures also
reach `EngineStatus.audio_error`.

## 8. Tests

- **Rust** (`os/shared/audio_meters.rs`, `os/shared/engine.rs`,
  `os/shared/obsws.rs`): event parser (level vs peak per channel), decay, peak
  hold, inactive snapshot, gain clamp 0–100, `percent_to_mul`, collection gains.
- **Frontend** (`pnpm test`, vitest): `clampPercent`, dB mapping
  (`levelToPercent`/`percentToLevelDb`), `levelToDb`/`percentToDb`, meter zones.

## 9. Secrets

No secrets are involved in the audio path. Reminder (see `05_STORAGE_SECURITY.md`):
never commit credentials, tokens or webhook URLs — the repository is public.
