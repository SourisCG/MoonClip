# 04 — Editor Pipeline (Quick trim + Advanced editor)

Two separate pieces, by design:

1. **Quick trim** — light panel inside the gallery. No timeline, no editor
   chunk, no background process: `<video>` + In/Out handles + one ffmpeg run.
2. **Advanced editor** — a Medal-style editor that opens **as a maximized view
   inside the app window** (the window is maximized on open and the previous
   size is restored on close). It is lazy-loaded: `React.lazy()` chunk that
   only exists while the editor is open, and all its libraries (timeline,
   canvas transforms, waveforms, undo) load with it. Nothing of the editor
   runs or is resident when it is closed.

## 1. Quick trim (Phase E1)

- Rust `editor/trim.rs` builds one ffmpeg argument list (pure, unit-tested):
  - `lossless` (default): `-ss IN -i src -t DUR -c copy -avoid_negative_ts make_zero`.
    Sub-second; the head snaps to the previous keyframe (documented 0–2 s
    tolerance, same rule as LosslessCut).
  - `precise`: `-map 0:v:0 -map 0:a? -c:v libx264 -preset veryfast -crf 18
    -c:a aac -b:a 192k` — frame-exact and **keeps every audio track**
    (Mix/Game/Mic). libx264 is in both bundled GPL sidecars.
- `-progress pipe:1` → `moonclip://edit-progress` events; cancel by dropping
  the child (future). Output is a NEW clip (`<stem>_trim.mp4`), indexed with
  probe + thumbnail and emitted as `moonclip://clip-saved`.
- UI `components/gallery/TrimPanel.tsx`: video preview, dual-handle bar,
  loop-inside-selection playback, precise toggle, progress bar.
- Acceptance: lossless trim of a real clip produces the cut in <1 s; precise
  is within one frame; the three audio tracks survive (probe).

## 2. Advanced editor (Phases E2–E6)

### Architecture

- **In-app maximized view** (like Medal's "Open in Editor"): switching to the
  editor fills the app window; closing unmounts the chunk and calls
  `cleanup_editing_session` (temps, running exports). RAM returns to gallery
  baseline.
- **Project** = versioned JSON (serde) stored in app data (`edits/<id>.json`),
  autosaved with debounce; reopening restores the timeline. Exports write a
  new clip into the library.
- **Preview**: `<video muted>` (hardware-decoded by the WebView) + DOM
  overlays in normalized coordinates with `react-moveable` handles; multi-clip
  uses a virtual player that swaps sources at cuts; transitions previewed as
  canvas/DOM cross-fades.
- **Media playback on Linux:** `asset://` cannot play audio/video on
  WebKitGTK (WebKit bug 146351), so `editor/media_server.rs` serves clips over
  `http://127.0.0.1:<ephemeral>/m/<token>` with HTTP range support. The
  server starts on the first media request (never at boot), binds loopback
  only and serves exclusively paths the backend registered.
- **Audio**: the 3 tracks (Mix/Game/Mic) are extracted once per clip to
  `~/.cache/MoonClip/editor/<session>/` and shown as 3 synchronized
  waveforms (`wavesurfer.js`) with per-track volume/mute/solo + music/uploads.
  **Export always mixes down to ONE AAC track** (Medal parity), respecting the
  editor mix. Temps are purged on close and stale sessions on boot.
- **Export**: Rust builds a `filter_complex` graph and runs the ffmpeg sidecar
  with `-progress pipe:1` and cancel-by-kill; encoder chosen per vendor
  (`NVENC/QSV/AMF/VAAPI`, libx264 fallback). Non-WebView-playable sources
  (HEVC/AV1) get an H.264 proxy for preview only; exports always use the
  original.

### Tools (licenses audited in `THIRD_PARTY.md`)

| Tool | License | Role |
|---|---|---|
| `dnd-timeline` (headless, dnd-kit) | MIT | Timeline rows/items, resize, snapping, time axis, pan/zoom, drag-to-create |
| `react-moveable` | MIT | Preview transform handles (drag/resize/rotate/snap/group) |
| `wavesurfer.js` | BSD-3 | Stems + music waveforms and regions |
| `zustand` + `zundo` + `immer` | MIT | Editor store + undo/redo |
| `react-colorful` | MIT | Color pickers |
| `@fontsource/*` | OFL | Bundled fonts for preview and libass `fontsdir` |
| `lucide-react` | ISC | Icons |
| ffmpeg sidecar (BtbN GPL) | GPL | Render: drawtext/libass, overlay, zoompan, rotate, chromakey, eq, xfade, amix, gif |

Discarded: GES/GStreamer (packaging + native preview sink on Wayland), MLT
(bindings abandoned), libav linking (sidecar rule), Konva (DOM + Moveable is
enough), in-browser WebCodecs (optional future accelerator only).

### Medal → ffmpeg mapping

trim/split/duplicate → `trim/concat`; speed → `setpts/atempo`; freeze →
`tpad/loop`; zoom/pos/rotation keyframes → `overlay/zoompan/rotate` with `t`
expressions; crop → `crop/scale`; filters → `eq/curves/colorbalance/hue`;
chroma/alpha → `chromakey/despill/format=rgba`; text → **ASS/libass** (font,
color, stroke, highlight, `\fad/\move/\t`); stickers/GIFs → `overlay` with
`enable` + expressions (GIF as its own input); transitions → `xfade`; audio →
`volume/amix/pan/adelay` to **1 track**; export → `zscale` + resolution/FPS/
bitrate preset.

### GIF search (no registration)

`Openverse` (anonymous: 1 req/s, page ≤ 20, CC-licensed GIFs) with Wikimedia
Commons fallback. Author/license is shown in the picker (CC-BY requires
attribution). Giphy/Tenor/Klipy are out: all require an API key and Google
**shut down the Tenor API in June 2026**.

### Sub-phases

- **E1** Quick trim (done when lossless <1 s, precise exact, 3 tracks kept).
- **E2** In-app maximized editor view + project JSON + single-clip timeline
  (trim/split/duplicate, undo/redo, Ctrl+K/S/D) + export presets with
  progress/cancel.
- **E3** Overlays: text (all properties + entrance/exit/effects), local
  stickers/images/GIFs, upload, Openverse search, Moveable interactions.
- **E4** Effects: speed 0.25x+, freeze, zoom/crop/rotate keyframes, filters,
  chroma, opacity.
- **E5** Multi-clip + transitions + 3-stem mixing and music → 1-track export.
- **E6** Polish: autosave/reopen, full hotkeys, complete export dialog, E2E
  and cross-platform close (Linux + Windows).

Every sub-phase: Rust golden-arg tests for the graph, `cargo test/clippy`,
`pnpm build`, and an E2E export of a 3 s clip asserting 1 video + 1 audio.

## 3. References (study only, never copy)

- **Cap (CapSoftware/Cap, AGPL-3.0):** Tauri+React+FFmpeg architecture. AGPL
  code cannot be relicensed into this GPL-3 project: read for ideas only.
- **LosslessCut (mifi/lossless-cut, GPL-3.0):** exact cut args, keyframe
  handling.
- **Medal editor (proprietary):** UX target only — in-app maximized editor,
  multi-track timeline, text/stickers/effects, single-audio export.
