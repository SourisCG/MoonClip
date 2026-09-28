# 03 — Game Detection (registered windows)

> Replaces the old process-rule detector (SteamAppId/Wine/launchers/custom
> apps). Since V3.6.5 detection is **window identity**: the user registers a
> game once with the OS picker, MoonClip stores the window title + app id, and
> the autopilot starts/stops the buffer while a matching window exists.

## 1. How it works

```text
1. User registers a game once ("Games → Register game"): the OS picker returns
   the window identity (title + app id/class + exe when available).
2. Every 3 s the autopilot lists desktop windows and matches them against the
   registered inputs (exact title, state suffix, tolerant app id).
3. A match starts the replay buffer if it is not running (or switches inputs);
   the buffer stops when the window is gone.
```

- `poll_games` (Rust) runs from `lib.rs` on a 3 s loop. Any window match with a
  registered input starts that input; if the buffer is already running with it
  nothing happens; if another registered input is going to be used the buffer is
  restarted once for the new source.
- Manually stopping the buffer wins: `suppressed_input` blocks the autopilot
  until that window goes away and comes back (leaving and re-entering the game
  re-arms it).
- `last_game_input` is remembered so the Start button (and the autopilot when
  several windows match) prefers the most recently played game.
- `current_game` exposes the active label to the UI.

## 2. Registration data

Registered inputs are rows of the (legacy-named) `custom_apps` table, extended
by migrations 011/012/013:

| Column | Meaning |
|---|---|
| `id` | Row id (vault alias for the portal token is `portal_<id>`) |
| `display_name` | User-facing game name (the picker's window title) |
| `input_name` | OBS source name (`Game 1`, `Game 2`, …) |
| `input_kind` | `window` (game) or `screen` (internal full-screen input) |
| `input_settings` | OBS source settings JSON (window target, monitor, sanitized) |
| `source_uuid` | Stable OBS source uuid |
| `window_title` / `window_app_id` | Identity learned at pick time (used by the matcher) |
| `clips_folder` | Library folder for this game ("" until the first pick) |
| `icon_path` | Optional icon |

Portal `RestoreToken`s inside `input_settings` are stripped before persisting
and live in the OS vault (`portal_<input_id>`), see `05_STORAGE_SECURITY.md`.

Commands: `register_game`, `edit_game` (clears the stored target and re-opens
the picker), `delete_registered_input` (keeps the clips folder and its clips
forever), `list_registered_inputs`.

## 3. Window lists per platform

`os::list_windows()` returns `DesktopWindow { title, app_id }`:

- **Linux + KDE (Wayland or X11):** KRunner's window list (title + icon/app id).
  Note it reports an **empty app id** for many apps (Brave, Discord, VS Code),
  which is why title guards exist.
- **Linux, other compositors:** X11/XWayland `_NET_CLIENT_LIST` → `_NET_WM_PID`.
- **Windows:** `EnumWindows` (title + window class + exe).

The picker returns richer identity (`WindowIdentity { title, app_id, exe }`):
on Linux it reads KDE's restore data (`steam_app_<id>`), on Windows the OBS
window-capture target (`"<title>:<class>:<exe>"`, parsed by
`parse_windows_target` — titles may contain colons, so it splits from the
right).

## 4. Matching rules (`os/shared/winlist.rs`, OS-free)

Shared by both platforms and unit-tested on every OS.

- **Normalization:** collapse whitespace, lowercase, drop trailing title
  separators (`-`, `|`, `:`, `–`, `—`) — game titles append state
  ("Terraria - 1.4.4.9", "Game | 1.2").
- **Title match:** normalized equality or word-boundary prefix (so state
  suffixes do not break it).
- **Guard 1 — file managers:** `is_non_game_window` rejects Dolphin, Nautilus,
  Thunar, Nemo, PCManFM, Krusader, Explorer classes (`cabinetwclass`,
  `explorewclass`) so browsing the clips folder never fakes a game.
- **Guard 2 — known non-game suffixes:** a title ending in
  `" - <app>"` / `" — <app>"` / `" – <app>"` for a known app (browsers, Dolphin,
  Discord, VS Code, Kate/KWrite, Spotify, Thunderbird, Steam) is rejected before
  any matching. Games append state, not app names.
- **App id compatibility (`app_id_matches`):** the picker (`steam_app_2357570`)
  and KRunner (`steam_icon_2357570` / `steam`) disagree in format, so ids match
  when equal, either contains the other, or the normalized Steam ids agree. If
  either side is unknown, the title decides.

## 5. Manual controls

- **Start buffer** (`start_buffer` → `manual_start`): picks a running registered
  game (preferring `last_game_input`) and starts it; it never records the screen
  (register a game for that). With no registered games it errors with a pointer
  to Games.
- **Record screen** (`start_screen_buffer`): records the internal `MoonClip
  Screen` input (screen-only capture, no game matching).
- The hotkey (`handle_hotkey`/F9) saves a clip when the buffer is running.

## 6. Per-game folders

Every registration owns a library folder (`<clips_dir>/<sanitized name>/`,
`storage/folders.rs`); the association is stable across renames and deletions
and is documented in `05_STORAGE_SECURITY.md` §7. `Unknown` holds clips saved
with no matching game.
