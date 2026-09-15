# 04 — Editor Pipeline (Lazy + FFmpeg)

Goal: Medal-style trim (In/Out) with waveform. No NLE. Preview in web, processing in Rust+FFmpeg.

## 1. References (study, do not copy blindly)

- **Cap (CapSoftware/Cap, AGPL-3.0):** Tauri+React+FFmpeg architecture, player↔Rust IPC layout.
- **LosslessCut (mifi/lossless-cut, GPL-3.0):** exact FFmpeg cut args, keyframe handling.
- MoonClip is GPL-3.0-compatible (due to `gpu-screen-recorder` GPL-3.0), so studying both is license-safe. Prefer MIT/Apache libs at runtime (`wavesurfer.js` BSD-3).

## 2. Frontend: `ClipEditor.tsx` (must be lazy)

```tsx
const ClipEditor = lazy(() => import('./components/editor/ClipEditor'));
// <Suspense fallback={...}><ClipEditor clip={editingClip} onClose={()=>setEditingClip(null)} /></Suspense>
```

- `<video src={convertFileSrc(clipPath)} muted={false}>` — HW-decoded by WebView (NVDEC/VA-API).
- `wavesurfer.js v7 + RegionsPlugin`: one region draggable/resizable, `region.on('update-end')` → `onRangeChange(start,end)`.
- Dual-track preview problem: HTML `<video>` plays only track 0:1. Solution: on open, Rust extracts audios to temp (`/tmp/moonclip/*.aac`), React loads **two** Wavesurfer instances (game + mic) with independent volume/mute sliders; keep `<video muted>` and sync web audio to `video.currentTime`.
- Cleanup (mandatory):
  ```tsx
  useEffect(() => {
    const ws = WaveSurfer.create({...}); ws.load(url);
    return () => { ws.destroy(); };
  }, [clip.id]);
  // + invoke('cleanup_editing_session')
  ```
- Alternative isolation: secondary Tauri window `editor-window` + `window.destroy()` on close (kills WebView process).

## 3. Backend: `editor/ffmpeg.rs` (sidecar CLI, no libav linking)

Sidecar binary per arch in `src-tauri/binaries/` (`ffmpeg-x86_64-pc-windows-msvc.exe`, `ffmpeg-x86_64-unknown-linux-gnu`). Static builds: BtbN (Win), johnvansickle musl (Linux). Keeps LGPL/GPL boundary at process level.

Commands:

```bash
# Thumbnail
ffmpeg -ss 00:00:01 -i input.mp4 -vframes 1 -q:v 2 thumb.jpg
# Temp audio extract (track layout is 1=mix, 2=game, 3=mic — NOT game/mic)
ffmpeg -i clip.mp4 -map 0:1 /tmp/audio_mix.aac -map 0:2 /tmp/audio_game.aac -map 0:3 /tmp/audio_mic.aac
# Lossless trim (default landscape)
ffmpeg -ss {IN} -to {OUT} -accurate_seek -i input.mp4 -c copy output.mp4
# Keep only game track
ffmpeg -i clip.mp4 -map 0:v -map 0:1 -c copy export.mp4
# Remix volumes for social (single stereo, from game+mic stems 0:2/0:3)
ffmpeg -i clip.mp4 -filter_complex "[0:2]volume=0.7[a1];[0:3]volume=1.5[a2];[a1][a2]amix=inputs=2:duration=longest[aout]" -map 0:v -map "[aout]" -c:v copy -c:a aac export_social.mp4
# Vertical 9:16 blurred (TikTok/Shorts, HW encode per vendor: nvenc/qsv/amf —
# see os::transcode_encoder; never assume nvenc on user machines)
ffmpeg -ss {IN} -to {OUT} -i input.mp4 -lavfi "[0:v]scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920,boxblur=20:5[bg];[0:v]scale=1080:-1[fg];[bg][fg]overlay=(W-w)/2:(H-h)/2" -c:v h264_nvenc -preset p4 output_tiktok.mp4
# Use h264_nvenc / h264_qsv / h264_amf (or hevc variants) depending on probe; fallback libx264 only if no HW.
```

## 4. Keyframe (I-frame) gotcha

H.264/H.265 stores full images only on keyframes (every 1–2s). Cutting `-c copy` off-keyframe → frozen/black head.

- MVP: `-accurate_seek` (seek to prior keyframe, no hang) + document 0–1s tolerance.
- Optional smart-cut: re-encode head GOP only. Simpler: full HW re-encode for <60s clips (~1.5s with NVENC/VA-API) when ms-exactness required.

## 5. Acceptance (Phase 5)

- [ ] Landscape trim exports <1s, no quality loss (`-c copy` verified via `ffprobe`).
- [ ] Vertical preset produces centered 1080x1920 with blurred bg via HW.
- [ ] Closing editor returns RAM to gallery baseline (no Wavesurfer leak, temps purged).
