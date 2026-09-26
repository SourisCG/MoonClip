# 03 — Game Detection (Steam, Wine/Proton, Launchers, Custom Apps)

> **As-built (V4, 2026-09-26).** The sections below remain the source
> reference; this is what shipped:
>
> - **Scanner**: `os/linux/detect.rs` (read-only `/proc`: exe/cmdline, GPU FDs
>   `/dev/dri/renderD*` + `/dev/nvidia*`, strict Wine detection, `SteamAppId`
>   env, flatpak cgroups, `x11rb` pid-to-window map) and
>   `os/windows/detect.rs` (ToolHelp32 + foreground window).
> - **Parsers** (`os/shared/detect/parsers.rs`): Steam `.acf` +
>   `libraryfolders.vdf`, Heroic `installed.json`, Prism `instance.cfg` +
>   `--gameDir`, Wine cmdline/exe, Minecraft Java, flatpak id, blacklist
>   (our engine, compositors, browsers/Electron, Wine plumbing).
> - **Resolution** (`resolve.rs`): custom app -> Steam -> Heroic ->
>   Prism/Minecraft -> Wine -> fallback (window title / exe stem). Windows
>   adds Steam-installdir, Epic manifests and a Battle.net exe map.
> - **State** (`custom_apps`, migration 010): `game_key` (stable identity),
>   `capture_mode`, `source_kind` (`window`/`portal`), `window_match`
>   (`id\r\nname\r\nclass` for `xcomposite_input`), `portal_token`,
>   `auto_buffer`, `last_seen_ms`, `icon_path`.
> - **Worker** (`commands::detect_tick`, 3 s): emits
>   `moonclip://game-changed` and drives the pure auto-buffer state machine
>   (`auto.rs`: 3 s start debounce, 7 s stop debounce, manual sessions never
>   auto-stopped, unknown games need the one-time registration).
> - **Capture**: X11/XWayland windows via `xcomposite_input` (zero dialogs,
>   survives restarts); Wayland-native windows via the portal with a per-game
>   restore token refreshed after every Start; monitor stays a manual option.
> - **Icons**: Steam `librarycache`, Prism `icons/<iconKey>`, Linux
>   `.desktop` theme icons -> cached PNGs under `<data>/MoonClip/icons`;
>   letter avatar fallback.
> - **Not implemented yet**: click-to-pick-window crosshair, Xbox/UWP and
>   EA/Ubisoft/Rockstar/GOG/itch manifests on Windows, exe icons via
>   `SHGetFileInfo`, and the real-game spike (KDE window restore behaviour).
>   See `docs/10_GAME_DETECTION_PLAN.md` for the live checklist.

## 1. Priority pipeline (on F9)

```text
1. custom_apps table match? → use custom display_name + custom duration
2. SteamAppId in env? → appmanifest_<id>.acf → official name
3. Wine/Proton? → .exe from cmdline (blacklist filtered)
4. Fallback → active window title / top GPU process
```

Poll worker every 3–5s with `sysinfo` (<0.1% CPU). Cache result in memory.

## 2. Linux GPU filter (kill 400-process noise)

A real 3D game holds an FD to `/dev/dri/renderD*` (AMD/Intel) or `/dev/nvidia*` (proprietary).

```rust
pub fn is_using_gpu(pid: u32) -> bool {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{}/fd", pid)) else { return false };
    for e in entries.flatten() {
        if let Ok(t) = std::fs::read_link(e.path()) {
            let s = t.to_string_lossy();
            if s.contains("/dev/dri/renderD") || s.contains("/dev/nvidia") { return true; }
        }
    }
    false
}
```

Then blacklist compositors/browsers: `gnome-shell, kwin, firefox, chrome, discord`.

X11/XWayland helper: query `_NET_CLIENT_LIST` → `_NET_WM_PID` via `x11rb` to list only PIDs with visible windows. 98% of Wine/Proton games go through XWayland.

## 3. Wine / Proton / Lutris

Process name is `wine-preloader`, `wine64-preloader`, `pressure-vessel`. Real exe is in `/proc/<pid>/cmdline` (NUL-separated, flags never glue to exe).

Example: `gamemoderun %command% -novid +fps_max 0` → elements `[gamemoderun, wine64-preloader, .../eldenring.exe, -novid, ...]`. Just find element ending in `.exe` (case-insensitive).

Blacklist: `winedevice.exe, explorer.exe, services.exe, conhost.exe, plugplay.exe, wineboot.exe, steam.exe, steamservice.exe`.

```rust
pub fn get_steam_app_id(pid: u32) -> Option<u32> {
    let bytes = std::fs::read(format!("/proc/{}/environ", pid)).ok()?;
    for var in bytes.split(|&b| b == 0) {
        if var.starts_with(b"SteamAppId=") {
            if let Ok(s) = std::str::from_utf8(&var[11..]) {
                if let Ok(id) = s.parse::<u32>() { return Some(id); }
            }
        }
    }
    None
}
```

Wrappers (`mangohud`, `gamemoderun`, `gamescope -- %command%`) inherit `SteamAppId` via fork/exec, so env survives. Only `env -i` breaks it (nobody does that — breaks Steam overlay).

Resolve name offline: `~/.steam/steam/steamapps/appmanifest_<id>.acf` (+ `libraryfolders.vdf` for secondary libs) → `"name" "ELDEN RING"`. No network needed. Optional: Steam Store API fallback.

Flatpak: read `/proc/<pid>/cgroup` → `app-com.valvesoftware.Steam-...`, `app-net.lutris.Lutris-...`.

## 4. Launchers

| Launcher | Detection |
|---|---|
| Steam (Win+Linux) | Linux: `SteamAppId` + `.acf`. Windows: `HKCU\Software\Valve\Steam\SteamPath` + `libraryfolders.vdf`, match exe. |
| Battle.net (Win native) | `C:\ProgramData\Battle.net\Agent\product.db`, exes `Overwatch.exe, Wow.exe, Diablo IV.exe`. Linux (Lutris/Bottles/Proton): ignore `Battle.net.exe`, detect GPU-holding child `.exe`. |
| Epic (Win) | `C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests\*.item` JSON → `DisplayName` + `LaunchExecutable`. |
| Heroic (Win+Linux) | `~/.config/heroic/GamesConfig/*.json`, `installed.json` → formal name + exe. |
| Minecraft Java (official) | `java` / `javaw.exe` + cmd contains `net.minecraft.client.main.Main`. Covers Forge/Fabric. |
| Prism Launcher | Extra arg `--gameDir .../instances/<Name>` → read `instance.cfg` for exact instance name. |
| Bedrock (Win only) | `Minecraft.Windows.exe` exact match (UWP). |
| Xbox / Game Pass (Win) | `gamelaunchhelper.exe` or path `XboxGames/` → Win32 `GetWindowTextW` on foreground window (e.g. "Forza Horizon 5"). |
| Proton generic | Same as Wine + SteamAppId. |

## 5. Custom apps (`custom_apps` table)

```sql
CREATE TABLE IF NOT EXISTS custom_apps (
  id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  target_exe TEXT NOT NULL,
  match_strategy TEXT NOT NULL, -- 'exact_exe' | 'cmdline_contains' | 'window_title' | 'wine_target'
  clip_duration_seconds INTEGER,
  icon_path TEXT,
  is_wine_proton INTEGER DEFAULT 0
);
```

UX (frontend `AppManager.tsx` + `ProcessPicker.tsx`):
- A. **From running processes:** `invoke('get_running_applications')` returns GPU/window-filtered list (`pid, name, display_name, cmdline`). Click `[+]` to bind. Special-case Java→"Minecraft (Java Edition)".
- B. **Click-to-register window:** minimize, crosshair cursor, `WindowFromPoint→GetWindowThreadProcessId` (Win) / `x11rb _NET_WM_PID` (X11/XWayland).
- C. **Browse file:** `plugin-dialog` filter `*.exe` (Win) or ELF/`.sh`/`*.exe` in `~/.local/share/wineprefixes/`, `~/.var/app/net.lutris.Lutris/` (Linux).

Matcher:

```rust
pub enum MatchStrategy { ExactExe(String), CmdlineContains(String), WineProtonTarget(String) }
pub fn matches_process(process: &sysinfo::Process, rule: &MatchStrategy) -> bool { /* ... */ }
```

Custom duration per app (fighters 15s, shooters 30s, RPG 60s, work/bug 15s). Fallback to global default.
