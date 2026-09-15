# THIRD_PARTY — Bundled components and license compliance

MoonClip is `GPL-3.0-only` (see `LICENSE`). This file tracks every third-party
component shipped inside MoonClip installers and what the GPL requires for each.

## gpu-screen-recorder (Linux capture engine)

- **What:** `gpu-screen-recorder` CLI + `gsr-kms-server` helper, built with
  `-Dcapabilities=false -Dffmpeg_static=true` (same recipe as upstream Flathub).
- **Upstream:** https://git.dec05eba.com/gpu-screen-recorder/ by dec05eba.
- **License:** `GPL-3.0-only` (per Arch, Alpine, Artix packaging). Compatible:
  MoonClip as a whole is GPL-3.0-only.
- **Pinned source (CI builds from this):**
  GitHub mirror `https://github.com/antonlobanovskiy/gpu-screen-recorder`
  (upstream git is bot-walled; mirror documents its upstream base commit)
  at commit `a2cfc66419fd7c814ab46e330592c983404b059b` (base: upstream 5.15.1).
  Built via `build-aux/build-gsr.sh` (meson release, `-Dcapabilities=false
  -Dsystemd=false -Dnvidia_suspend_fix=false -Dstrip=true`), output tagged
  with the rustc target triple under `src-tauri/binaries/<triple>/`.
  Bump the pin by updating `GSR_COMMIT` in that script.
- **Source offer (GPL §6):** Corresponding Source = pinned snapshot above
  plus our build flags (this file + the script). No modifications to GSR itself.
- **Ship model per format:**
  - `.rpm` / `.deb` / `.AppImage`: prebuilt sidecar under
    `<app>/moonclip-gsr/`; system deps (`libdrm`, `libva`, pipewire, pulse)
    come from official distro repos via `Requires`/`Depends`.
  - Flatpak/Flathub: `gpu-screen-recorder` compiled as a manifest module
    (mirrors upstream manifest), `finish-args: --device=all
    --socket=pulseaudio --socket=wayland --socket=fallback-x11`.
  - Windows: not shipped (native WGC/AMF/NVENC APIs instead).
- **Runtime resolution order:** `MOONCLIP_GSR_BIN` override → bundled sidecar
  → system `PATH` → clear error. Dev machines use a native install
  (Terra/COPR rpm on Fedora); end users never install anything extra.
- **Windows ships NO GSR.** Capture there is native (WGC/DXGI + WASAPI, see
  `09_WINDOWS_HANDOFF.md`); `os::backend_binary` reports that by design.
  The only Windows sidecar is static FFmpeg (BtbN).

## FFmpeg (editor pipeline + Windows capture engine)

- **What:** `ffmpeg` CLI sidecar for thumbnails, lossless cuts, vertical presets
  (Linux editor + saver) and for the whole Windows capture path (live
  encode, save mux, codec probes).
- **Upstream:** https://ffmpeg.org (static builds: BtbN for Windows,
  johnvansickle musl for Linux).
- **Pinned Windows build (CI/dev fetch from this):**
  BtbN `win64-gpl` monthly `autobuild-2026-08-31-13-27`,
  asset `ffmpeg-N-126342-gf88b741dbf-win64-gpl.zip`
  (`sha256:b4da332540eaebc6939181b59e267f163dd57407ef6596f7f3452845921d1d91`,
  ~163 MB download). Fetched via `build-aux/fetch-ffmpeg.ps1` (verifies
  hash + required encoders: nvenc h264/hevc/av1, amf h264/hevc, qsv
  h264/hevc, libx264, aac), staged at
  `src-tauri/binaries/x86_64-pc-windows-msvc/ffmpeg-x86_64-pc-windows-msvc.exe`
  (gitignored, like GSR). Bump the pin by updating the script defaults.
- **Ship model:** `tauri.conf.json > bundle.resources` includes the staged
  exe on Windows builds (per-OS CI config overlay — the base conf stays
  platform-neutral so Linux bundles never carry the `.exe`). Resolved at
  runtime via `sidecar::search_bundled` (`editor::ffmpeg::resolve_ffmpeg`);
  PATH is a dev-only fallback with a loud log. No shell-plugin sidecar:
  the engine needs raw binary pipes, not line events.
- **Dev fallback:** system `ffmpeg` from `PATH` when no sidecar is present,
  with a log warning. Production always ships the pinned sidecar.
- **License:** the static builds we ship (BtbN for Windows, johnvansickle
  musl for Linux) are GPL builds — they link `libx264`/`libx265`, which
  MoonClip needs for the `x264` CPU fallback codec (Windows `offered_codecs`
  always lists it; save-time `TranscodeEncoder::X264` maps to `libx264`).
  This is compliant because MoonClip as a whole is already `GPL-3.0-only`
  (via GSR, above). FFmpeg stays a separate process (CLI boundary); no
  libx264 code is linked into MoonClip itself.
- **Dev fallback:** system `ffmpeg` from `PATH` when no sidecar is present,
  with a log warning. Production always ships the pinned sidecar.

## Reference code (NOT shipped)

- Cap (AGPL-3.0) and LosslessCut (GPL-3.0): architecture/FFmpeg-args reference
  only. No code copied. If any snippet is ever adapted, its origin and license
  must be recorded here.
