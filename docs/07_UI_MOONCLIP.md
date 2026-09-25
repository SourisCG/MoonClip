# 07 — MoonClip UI (Nocturnal + Zero-Cost While Gaming)

Inspired by https://moonclip.souriscg.dev / https://github.com/SourisCG/MoonClip (C starfield). Web re-implementation must cost 0 GPU/CPU while gaming.

## 1. Palette (Tailwind v3, see `tailwind.config.js`)

- Base void: `#050608` (app bg), panels `#0b0f19/50` + `backdrop-blur-xl border-white/5`.
- Cards: `#0d1220/60`.
- Accents: lunar `#38bdf8` → astral `#818cf8`, starlight `#e0e7ff`.
- Glow: `shadow-[0_0_15px_rgba(56,189,248,0.3)]`, hover borders `hover:border-cyan-500/40`.
- Top aura: `w-[700px] h-[250px] bg-gradient-to-b from-cyan-500/10 via-indigo-500/5 to-transparent blur-3xl`.

Already in `tailwind.config.js` as `colors.moonclip.{void,panel,card,lunar,astral,starlight}`.

## 2. Starfield: `src/components/starfield/MoonClipStarfield.tsx`

- `<canvas fixed inset-0 pointer-events-none z-0 opacity-70>`, 70–100 stars (spec: 85), size 0.4–2px, `sin(frame*speed)*0.35` twinkle, glow on `size>1.4`.
- **Critical:** pause on hidden:
  ```tsx
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) { isPaused=true; cancelAnimationFrame(id); }
    else if (isPaused) { isPaused=false; render(); }
  });
  // + window blur/focus, + unmount cleanup
  ```
- No CSS-div stars, no uncontrolled rAF. Implement in Phase 1.

## 3. Layout (Phase 1 target)

- Sidebar `w-64 rounded-2xl bg-[#0b0f19]/50 backdrop-blur-xl border-white/5`: logo (🌙 gradient), nav (My Clips, Games, Settings), live status (pulsing dot "Replay Buffer Active / F9").
- **Responsive (portrait/second screens)**: window min `420x420` fits a 768-wide rotated monitor. Sidebar collapses to a 56 px icon rail below `lg` (1024) — logo, nav icons with `title` tooltips, status dot + icon-only start/stop; hotkey card and language row return at `lg` (language also lives in Settings). Main padding `p-3` → `sm:p-4` → `lg:p-6`. Gallery rows and settings/AppManager rows stack below `sm` (640), thumbs switch to `aspect-video` full width; topbar wordmark shows from `sm`, tagline from `md`. No new deps: Tailwind breakpoints only.
- Main `flex-1 rounded-2xl bg-[#0b0f19]/30 border-white/5`: gallery or editor outlet.
- Clip cards: `group rounded-xl overflow-hidden border-white/5 hover:border-cyan-500/40 hover:shadow-[0_0_20px_rgba(56,189,248,0.15)]`, `aspect-video` thumb + bottom gradient + `gameTitle` + `duration`.

## 4. i18n (bilingual ES/EN)

- `react-i18next` already installed. `src/locales/{en,es}.json`, detector = `settings.locale` (not browser-only). All strings via `t()` from Phase 1. Docs/README stay in English.

## 5. React perf note (why it won't lag games)

- While gaming, window is tray-hidden → WebView2/WebKitGTK pauses render, CSS, rAF. Capture lives in GSR/Rust VRAM, zero JS.
- React never processes pixels; `<video>`/`<audio>` are HW-decoded. Editor is lazy-unmounted (see `04_EDITOR_PIPELINE.md`).
- Tauri (~30–60 MB) vs Electron/Medal (300–800 MB): no bundled Chromium, Rust binary not Node.

## 6. Windows notes (WebView2) + tray/taskbar icons

- Renderer is WebView2 (Chromium) on Windows vs WebKitGTK on Linux: `<video>`
  H.264/AAC playback works in both; asset protocol scope
  (`$HOME/$RESOURCE/$TEMP`) resolves on both (see `tauri.conf.json`).
- Frameless custom topbar (`Topbar.tsx`): drag region, minimize/maximize/
  close-to-tray and edge/cube resize grips use Tauri window APIs only —
  cross-platform by construction. Never use CSS `app-region` hacks.
- Icons: window/taskbar set in `tauri.conf.json` (`icons/`, generated from
  `build-aux/common/moonclip-icon.svg` as transparent artwork); tray uses
  `icons/tray-icon.png` loaded explicitly in `lib.rs` (kept as a separate
  asset so tray and taskbar can evolve independently).
  Master artwork: `build-aux/common/moonclip-icon.svg`.
- Linux taskbar association needs TWO entries, one per context:
  - **Dev** (`tauri dev` / `target/debug`): `build-aux/common/dev.souriscg.moonclip.desktop.template`
    + `build-aux/linux/install-dev-desktop.sh` (exposed as `pnpm desktop:install`).
    Wayland matches the window by `appId` (`dev.souriscg.moonclip` via
    `app.enableGTKAppId`) against `StartupWMClass` + the `.desktop` filename —
    without it the taskbar shows the generic Wayland icon while the tray
    (explicit pixels) still renders.
  - **Packaging** (deb/rpm/AppImage, Phase 7): the bundler ships the
    production `.desktop` + icons — that part is still pending (see `08`).
