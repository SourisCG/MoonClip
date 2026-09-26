# 10 — V4 Plan: Game detection, window capture and auto-buffer

Live plan for Phase 4. `docs/03_GAME_DETECTION.md` holds the detection-source
reference (rewritten as-built in 4.9); this file is the execution contract:
one numbered sub-phase at a time, each with its own acceptance gate.

## Locked decisions

- **Window capture always** (the existing monitor/portal-screen option stays
  available as a manual choice in Settings → Video).
- **One window choice per game** — via two routes:
  - **X11 / XWayland** (`source_kind='x11'`): `xcomposite_input` with
    `capture_window = "<id>\r\n<WM_NAME>\r\n<WM_CLASS>"`. Zero dialogs, works
    across game restarts (name/class are stable). Covers Proton and most
    native titles that run under XWayland.
  - **Wayland native** (`source_kind='portal'`): the portal window session
    with a **per-game `RestoreToken`** stored in the DB and refreshed after
    every Start. KDE restores by app id + title (portal MR !310), so the
    picker appears once per game. If KDE invalidates the token, the user gets
    **one prompt per session** (fallback (a) chosen by the owner) and the new
    token is stored again.
- **Register once**: unknown games get a one-time prompt ("Detected X — add
  and record"). Rows persist forever in `custom_apps` and are deletable by
  the user (deleting forgets window/token; next use asks once again).
- **Medal-style auto-buffer**: default ON, global toggle plus per-app
  override; manual sessions are never auto-stopped.
- **Real icons**: Steam `librarycache`, Prism `icons/<iconKey>`, Linux
  `.desktop` theme icons, Windows `SHGetFileInfo`; cached under the MoonClip
  data dir; letter avatar as last resort.

## Architecture

```
os/shared/detect/{types,parsers,matcher,cache}.rs   OS-free logic + fixtures
os/linux/detect.rs     /proc scan, GPU FD, x11rb (pid<->window), manifests
os/windows/detect.rs   ToolHelp32, GetForegroundWindow, registry/manifests
os/mod.rs              new_detector(), running_applications(), current_game()
```

- Worker: tokio task polling every 3 s (<0.5 % CPU), emits
  `moonclip://game-changed` only on change; state in `AppState`.
- Blacklist: `moonclip-engine`, `moonclip-mux`, compositors (`kwin_wayland`,
  `gnome-shell`, `mutter`, Xwayland), browsers/Electron (`firefox`, `chrome`,
  `chromium`, `discord`, `steamwebhelper`, `code`), `ffmpeg`. Never
  `game_capture`; detection is read-only.
- Dev hook: `MOONCLIP_FAKE_GAME="<name>"` injects a candidate so the whole
  flow (chip, register prompt, auto-buffer, clip title) is testable without a
  real game.

## DB (migration 010)

`custom_apps` gains:

| column | meaning |
|---|---|
| `game_key TEXT UNIQUE` | `steam:<appid>`, `prism:<instance>`, `wine:<exe>`, `x11:<class>`, `exe:<path>` |
| `capture_mode TEXT DEFAULT 'window'` | `window` \| `monitor` |
| `source_kind TEXT` | `x11` \| `portal` \| `monitor` |
| `window_match TEXT` | x11 encoded id + name + class |
| `portal_token TEXT` | Wayland-native restore token for this game |
| `auto_buffer INTEGER DEFAULT 1` | per-app auto-buffer toggle |
| `last_seen_ms INTEGER` | diagnostics / stale cleanup |

Rows are auto-created for known games when there is state to store
(token/prefs) and for user-registered games. Deleting a row forgets that
state.

## Auto-buffer state machine

| Event | Action |
|---|---|
| known/registered game stable ~3 s, buffer off, auto enabled | silent start (x11 or stored token) |
| first capture of a game without token | pause, one window prompt, store, continue |
| unknown candidate | one-time toast: add & record; nothing starts until accepted |
| candidate gone ~7 s, session was auto | stop |
| manual session | never auto-stopped |

## Status (2026-09-26)

| # | Status | Commit |
|---|---|---|
| 4.0 docs | done | `93a9cef` |
| 4.1 Linux detection | done | `5db7c96` |
| 4.2 DB + matcher | done | `afd34c8` |
| 4.3 window capture | done | `e0d867b` |
| 4.4 auto-buffer | done | `ebf8538` |
| 4.5 clip integration | done | `2d4e2ce` |
| 4.6 icons | done | `eadd458` |
| 4.7 UX | done | `9b03d5f` |
| 4.8 Windows code | done (owner pass pending) | `d9336c6` |
| 4.9 close | in progress | - |
| Spike A/B/C | pending (real game) | - |

## Sub-phases (agent pattern: implement ONLY 4.x, verify acceptance, commit)

| # | Scope | Acceptance |
|---|---|---|
| 4.0 | Spike + docs refresh (this file, SPEC, ROADMAP, docs/03 header) | docs committed; spike checklist ready |
| 4.1 | Linux detection: `/proc` scan, GPU FD, blacklist, Wine/Proton, Steam `.acf` + `libraryfolders.vdf`, Heroic/Epic/Lutris, Prism/Minecraft, x11rb pid↔window | fixture-based unit tests (parsers + matcher priority + blacklist) |
| 4.2 | Matcher + migration 010 + persistence/delete + `get_running_applications` | DB tests; delete/restore behavior |
| 4.3 | Window capture wiring: `source_kind` selection, `window_match`, per-game token write/refresh | E2E with fake game (x11 and simulated portal) + real spike Tests A/B/C |
| 4.4 | Auto-buffer state machine integrated with `start_engine`/`stop` | unit states + E2E autostart/auto-stop |
| 4.5 | Clip integration: real title in `do_save_clip`, per-game duration (debounced restart), Gallery filter/group | E2E title + duration |
| 4.6 | Icons: Steam librarycache, Prism instance icon, `.desktop` theme, Windows `SHGetFileInfo`, cache dir | visual + fallback |
| 4.7 | UX: status chip, one-time register toast, Games ProcessPicker (running / pick window / browse), toggles | fake-game E2E |
| 4.8 | Windows: WGC window capture by title/class, foreground PID, Steam registry + ACF, Epic, Battle.net `Agent\product.db`, Xbox, icons | parser unit tests + owner's compile/in-game pass |
| 4.9 | Close: `detection-live-check.sh`, PROGRESS V4, docs/03 as-built, README | all scripts + owner's in-game pass |

## Spike procedure (after 4.3)

- **Test A (XWayland/Proton)**: detect game, capture window via xcomposite,
  close and reopen the game → still silent, no dialog.
- **Test B (Wayland native)**: first picker stores a token; close and reopen →
  does KDE restore silently? Record result; if not, confirm fallback (a)
  (one prompt per session) and note it in the row/tooltip.
- **Test C (two games)**: separate rows, sources and tokens; switching games
  never crosses windows.

Record results in the PROGRESS V4 entry.

## Risks

- KDE window restore behavior can change between Plasma releases (validate in
  the spike; the x11 route is unaffected).
- Minimized windows may freeze the last frame (OBS window capture); document
  it and keep the monitor option.
- Multiple GPU candidates (browser with HW accel) are handled by the
  blacklist; unknown apps only start after the one-time registration.
