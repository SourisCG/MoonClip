# 07 — UI (Medal-style shell, v2 design system)

> The UI was rebuilt (2026-09-28) around Medal-style navigation: a fixed icon
> rail, a contextual rail, a full-width header and flat surfaces. The old
> "MoonLit" glass/cyan look is gone. This document is the reference for the
> tokens, components and cross-webview rules.

## 1. Direction

- Neutral layered blacks (Medal-like), **no generic dark-app cyan/purple**.
- Accents come from the brand mark: **red** `#ef4444` (actions) and **blue**
  `#3b82f6` (links/focus); aqua is reserved for Aero-like glows.
- Contrast is measured, not guessed: body text is ≥ 4.5:1 on every surface.

## 2. Tokens (`tailwind.config.js` + `src/index.css`)

| Token | Value | Use |
|---|---|---|
| `base` | `#000000` | app background (root, header) |
| `surface` | `#161617` | rails, panels, cards |
| `raised` | `#1f1f20` | rows, chips, inputs, hover |
| `line` / `line-strong` | `rgba(255,255,255,.08)` / `.16` | hairlines and borders |
| `ink` | `#ffffff` | primary text (21:1) |
| `ink-soft` | `#b3b1b6` | secondary text (9.9:1) |
| `ink-muted` | `#8b8b90` | labels, meta (5.3:1) |
| `ink-faint` | `#6e6e73` | decorative only (3.2:1) |
| `brand` / `brand-bright` | `#ef4444` / `#f87171` | primary buttons, recording, playheads |
| `link` / `link-bright` | `#3b82f6` / `#60a5fa` | links, focus, selections, meters |
| `ok` / `warn` / `aqua` / `edit` | `#3fb950` / `#d29922` / `#39c5cf` / `#a371f7` | states and editor accents |

Radii: `rounded-control` (8 px) for controls, `rounded-card` (10 px) for panels.
Shadows: `shadow-panel` (floating surfaces), `shadow-pop` (popovers).
Legacy v1 aliases (`blood`, `gold`, `jade`, `sky`, `panel`, …) are kept in the
config only as a transition aid; new code uses the tokens above.

## 3. Fonts

- **Inter** (UI) and **JetBrains Mono** (metas, numbers, keycaps), packaged as
  local `woff2` in `public/fonts/` (latin + latin-ext) and declared in
  `src/fonts.css`; no network fetch, identical metrics offline and cross-engine.
- No serif display font anymore. Mono uppercase labels are the only "indie"
  detail that stays.

## 4. Shell (`src/components/shell/AppShell.tsx`)

```text
Topbar (frameless: drag region, window controls)
┌──────────┬───────────────┬──────────────────────────────────────────┐
│ IconRail │ ContextRail   │ Header (search / sort / actions)         │
│ 72 px    ├───────────────┴──────────────────────────────────────────┤
│ Library  │                                                          │
│ Games    │ Main (scrollable)                                        │
│ Settings │   gallery grid / games / settings                        │
│ …        │                                                          │
│ record   │                                                          │
└──────────┴──────────────────────────────────────────────────────────┘
```

- **IconRail**: Library, Games, Settings, plus the record status dot, the
  start/stop button, "record screen" and the ES/EN toggle. The active item
  carries a brand-colored side bar.
- **ContextRail**: changes per view — library filters (All/Favorites/Uploaded/
  In Drive + games with counts), registered games, or settings sections. It can
  be collapsed from the header (`PanelLeft` button); the state persists in
  `localStorage` (`moonclip.rail`).
- **Header**: global search, sort select, Drive browser, purge, "Save now" while
  buffering.
- **Main**: edge-to-edge scroll area, no nested cards.

## 5. Component kit (`src/components/ui/`)

`Button` (primary/secondary/ghost/danger, sizes), `Field` + `Input` +
`Checkbox`, **`Select`** (custom button + portaled popover — native selects
render differently per engine), `Card` + `CardHeader`, `Tag`, `Tabs`,
`Dialog` (portal, Escape, click-outside, focus restore), `Meter`,
`ProgressBar`, `Kbd`, `EmptyState`.

Rules:
- The modal system **must** portal into `document.body` (the shell uses
  `backdrop-filter`, which turns `fixed` descendants into scroll-away elements)
  and registers with `src/lib/overlay.ts` so the starfield pauses.
- `user-select: none` globally; inputs/textareas opt back in.

## 6. Gallery and clip panel

- Grid of 16:9 cards (`surface`, hairline, duration badge, cloud badge, hover
  actions). **Hover preview**: after a 260 ms dwell a muted looping video
  replaces the thumbnail (local clips only — cloud clips would trigger a full
  download).
- Clicking a card opens the clip panel (`TrimPanel`): player, metadata,
  inline rename, actions (share, advanced editor, favorite, reveal, delete with
  cloud confirmation) and ←/→ navigation. Cloud clips download on demand with
  progress and the temp copy is deleted on close.
- Both editors show bolder position indicators: the quick-trim timeline and the
  advanced timeline draw a 3 px brand-red playhead with a top handle and glow,
  and the advanced editor's scrub bar (under the video) has an always-visible
  round thumb plus its dB-free progress fill.

## 7. Starfield and background

- `MoonClipStarfield.tsx`: 85-star canvas, sinusoidal twinkle, warm-white stars
  with a soft gold halo. It pauses on `document.hidden`, window blur and while
  any overlay is open (`lib/overlay.ts`).
- `.low-power` (software compositing detected at boot) disables all
  `backdrop-blur-*` and hides the starfield entirely.

## 8. Cross-webview parity (WebKitGTK ↔ WebView2)

- Every interactive control is drawn by us: `appearance: none` + custom
  `Select`, checkbox SVG, `input[type=range].fader` (track/thumb pseudo
  elements), custom scrollbars (`::-webkit-scrollbar`), one focus ring
  (`:focus-visible`), `color-scheme: dark`.
- Bundled fonts for all text; no reliance on OS font fallbacks.
- The layout uses flex/grid only (no `-webkit`-specific hacks) and the same
  spacing scale on both platforms.

## 9. i18n, icons, tests

- All user-facing strings live in `src/locales/{es,en}.json` (identical key
  sets); docs are English-only.
- Icons: `lucide-react`.
- Frontend tests: `pnpm test` (vitest, `src/lib/audio.test.ts` and pure logic);
  gates are `pnpm build`, `cargo test`, `cargo clippy -D warnings`, `pnpm test`.

## 10. Secrets reminder

UI copy or settings must never contain credentials; `social.json` is read by
Rust only and never crosses IPC. Never commit secrets (see
`05_STORAGE_SECURITY.md` §Secrets policy).
