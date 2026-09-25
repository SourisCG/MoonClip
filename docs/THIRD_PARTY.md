# THIRD_PARTY — Bundled components and license compliance

MoonClip is `GPL-3.0-only` (see `LICENSE`). This file tracks every third-party
component shipped inside MoonClip installers and what the GPL requires for each.

## OBS Studio (Windows + Linux capture engine)

- **What:** the official OBS Studio distribution, shipped as an **isolated,
  embedded copy** used only by MoonClip. Replay-buffer capture + encode + mux
  are owned by this child process; MoonClip writes its profile/scene and drives
  it through its private obs-websocket from the app itself (`obws` crate).
- **Upstream:** https://github.com/obsproject/obs-studio
- **License:** GPL-2.0-or-later for OBS itself. Compatible: MoonClip as a
  whole is GPL-3.0-only; OBS is a separate process (CLI/IPC boundary), it is
  not linked into MoonClip.
- **Pinned source (CI/dev fetch from this):**
  - Windows: release `32.2.2`, asset `OBS-Studio-32.2.2-Windows-x64.zip`
    (`sha256:4d6e40e3ab155f56b30de517380566a206d74b63cdf5ad49aa596924768f97e1`,
    ~188 MB). Staged at `src-tauri/binaries/x86_64-pc-windows-msvc/obs/`
    with the portable layout `bin/64bit/obs64.exe` + `data/` + `obs-plugins/`.
  - Linux: release `32.2.2` **built from source** (OBS publishes no portable
    Linux tarball; the Ubuntu `.deb` depends on Ubuntu sonames). `build-aux/linux/build-obs.sh`
    clones the pinned tag with submodules, builds with a minimal plugin set
    (`-DENABLE_RELOCATABLE=ON`, browser/VLC/VST/scripting/AJA/decklink/WebRTC
    off) against the distro's Qt6/FFmpeg/PipeWire, and stages a relocatable
    tree (`bin/obs` + `obs-ffmpeg-mux` + `obs-nvenc-test`, `lib64/obs-plugins`,
    `share/obs`) at `binaries/x86_64-unknown-linux-gnu/obs/` with RUNPATH
    `$ORIGIN/../lib64`. The compiled-in install prefix is neutral
    (`/nonexistent/moonclip-obs`) so OBS never scans a second plugin path.
    Build cache: `~/.cache/MoonClip/obs-build` (ninja resumes).
  - Bump the pin by updating `build-aux/windows/fetch-obs.ps1` (Windows) /
    `build-aux/linux/build-obs.sh` (Linux) + `docs/THIRD_PARTY.md`.
- **Isolation guarantees (why the user's OBS is never touched):**
  - **Windows:** OBS Studio ignores `--config-dir` (verified live: it logged
    "Portable mode: false" and wrote to `%APPDATA%\obs-studio`). MoonClip
    therefore stages its own writable copy of the embedded OBS under
    `%LOCALAPPDATA%\MoonClip\obs`, writes `portable_mode.txt` and launches it
    with `--portable`. OBS writes `config/` inside that copy by construction.
  - **Linux:** the embedded build is relocatable and runs with
    `XDG_CONFIG_HOME=~/.local/share/MoonClip/obs/config`, so OBS writes its
    whole config/log tree there; `~/.config/obs-studio` is never touched.
    A system-`obs` fallback (dev) gets the same env treatment.
  - Common flags: `--multi`, `--profile MoonClip`, `--collection MoonClip`,
    `--minimize-to-tray`, `--disable-shutdown-check`, `--disable-updater`,
    `--only-bundled-plugins`.
  - The engine kills the child if it fails to create its isolated config
    (see `os/shared/engine.rs` config guard). The user's `%APPDATA%\obs-studio`
    / `~/.config/obs-studio` is never read or written.
  - obs-websocket binds `127.0.0.1` on a dedicated port with a generated
    password; only the bundled obs-cmd talks to it.
- **Anti-cheat:** the generated scene contains only the platform display
  capture source (`monitor_capture`/`window_capture` via DXGI/WGC on Windows,
  PipeWire portal on Linux). `game_capture` (the hooking source) is forbidden
  and asserted in tests. Audio comes from WASAPI loopback / PulseAudio.

## FFmpeg (editor pipeline + thumbnails/probes)

- **What:** `ffmpeg` CLI sidecar for thumbnails, duration probes, lossless cuts
  and vertical presets. It is NOT the capture engine anymore (OBS is).
- **Upstream:** https://ffmpeg.org (static builds: BtbN `win64-gpl` for
  Windows and `linux64-gpl` for Linux).
- **Pinned Windows build (CI/dev fetch from this):**
  BtbN `win64-gpl` monthly `autobuild-2026-08-31-13-27`,
  asset `ffmpeg-N-126342-gf88b741dbf-win64-gpl.zip`
  (`sha256:b4da332540eaebc6939181b59e267f163dd57407ef6596f7f3452845921d1d91`,
  ~163 MB download). Fetched via `build-aux/windows/fetch-ffmpeg.ps1` (verifies
  hash + required encoders), staged at
  `src-tauri/binaries/x86_64-pc-windows-msvc/ffmpeg-x86_64-pc-windows-msvc.exe`
  (gitignored). Bump the pin by updating the script defaults.
- **Pinned Linux build (dev/local installs):** BtbN `linux64-gpl`
  `autobuild-2026-09-15-13-18`, asset
  `ffmpeg-N-126574-g912208af28-linux64-gpl.tar.xz`
  (`sha256:7c5b4fd54be55970595f3c62b22c8a502587abe61c3e5e37a2cb81630093bad4`,
  151,537,292 bytes). Fetched via `build-aux/linux/fetch-ffmpeg.sh` (sha256 + size
  + encoders: libx264, h264_nvenc, hevc_nvenc, aac), staged at
  `src-tauri/binaries/x86_64-unknown-linux-gnu/ffmpeg-x86_64-unknown-linux-gnu`.
- **Ship model:** `tauri.windows.conf.json` (per-OS overlay) bundles the
  staged exe on Windows builds; `package.json` `tauri:build:linux` bundles the
  Linux sidecars. Resolved at runtime via `sidecar::search_bundled`
  (`editor::ffmpeg::resolve_ffmpeg`); PATH is a dev-only fallback with a loud
  log.
- **License:** the static builds we ship (BtbN `win64-gpl` and `linux64-gpl`)
  are GPL builds. MoonClip as a whole is already `GPL-3.0-only`. FFmpeg stays
  a separate process (CLI boundary); no FFmpeg code is linked into MoonClip.

## Removed components

- **obs-cmd:** removed (2026-09-23). Its latest release (v1.0.2) ships
  `input settings`, `input volume`, `input mute` and `audio mute` as stubs
  that only print "experimental" and never talk to OBS (portal token
  read-back, live gains/mutes were silently dead). MoonClip now talks
  obs-websocket v5 directly through the `obws` crate (MIT/Apache-2.0, Rust
  dependency, not a shipped binary): same protocol, plus the replay flush
  polling (obs-cmd issue #103) reimplemented in-process.
- **gpu-screen-recorder (GSR):** replaced by the embedded OBS engine in V3
  (2026-09-18). No GSR binary is bundled or resolved anymore; the
  `game_capture`-free OBS scene is the anti-cheat-safe capture path on both
  OSes. `build-aux/build-gsr.sh` was deleted.

## Reference code (NOT shipped)

- Cap (AGPL-3.0) and LosslessCut (GPL-3.0): architecture/FFmpeg-args reference
  only. No code copied. If any snippet is ever adapted, its origin and license
  must be recorded here.
