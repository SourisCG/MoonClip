# 05 — Storage & Security (SQLite + Keyring)

## 1. Rule: split data from secrets

| Type | Where | Tool | Security |
|---|---|---|---|
| Metadata (titles, dates, favorites) | Local disk DB | SQLite (`rusqlite`, backend-only) | Plaintext, fast SQL |
| Secrets (OAuth refresh tokens, webhook URLs, API keys) | OS vault | `keyring` crate | DPAPI/TPM (Win), Secret Service/KWallet (Linux) |

SQLite is ~1 MB embedded, ~0 RAM idle. Do NOT use SQLCipher with hardcoded key (decompilable in OSS). Do NOT ask master password on every clip (ruins UX).

Implemented (2026-09-27):

- **Portal `RestoreToken`s** are stripped from `custom_apps.input_settings`
  before persisting and kept in the OS vault (`portal_<input_id>`); they are
  merged back when the capture profile is built. A one-time startup migration
  moves tokens from older builds to the vault (plaintext-DB fallback only when
  no vault exists, so minimal WMs keep capturing).
- **The obs-websocket password is ephemeral**: generated per engine start,
  written only into that run's generated OBS config, never persisted. The DB
  scrub removes legacy `engine_ws_password`/`engine_restore_token` rows.
- **Hardening**: owner-only permissions on MoonClip-owned paths (app-data dir
  0700, `moonclip.db` + WAL/SHM 0600, `edits/`, editor sessions, generated
  engine config); the user's clips directory is never chmodded. SQLite runs
  with WAL + `foreign_keys=ON` + busy timeout and all pending migrations
  commit in one transaction. The webview has a real CSP and the unused
  `asset://` protocol (previously scoped to `$HOME/**/*`) is gone; media flows
  over IPC and the tokenized loopback server only.

## 2. Schema (relative paths only)

```sql
CREATE TABLE IF NOT EXISTS clips (
  id TEXT PRIMARY KEY,
  file_name TEXT NOT NULL UNIQUE,       -- 'clip_2026-09-05_cs2.mp4' (relative!)
  thumbnail_name TEXT NOT NULL,
  game_title TEXT NOT NULL,
  duration_ms INTEGER NOT NULL,
  file_size_bytes INTEGER NOT NULL,     -- for auto-pruning quota
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  is_favorite INTEGER DEFAULT 0,        -- 1 = protected from auto-delete
  drive_file_id TEXT,
  drive_web_url TEXT
);
CREATE INDEX IF NOT EXISTS idx_clips_created_at ON clips(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_clips_game ON clips(game_title);

CREATE TABLE IF NOT EXISTS custom_apps (
  id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  target_exe TEXT NOT NULL,
  match_strategy TEXT NOT NULL,
  clip_duration_seconds INTEGER,
  icon_path TEXT,
  is_wine_proton INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
-- settings: clips_directory, buffer_seconds, hotkey, max_storage_gb, locale,
-- gain_game, gain_mic, mute_game, mute_mic, mic_device, desktop_device,
-- video_codec, out_height, fps, monitor
```
Migrations live in `src-tauri/migrations/` (`PRAGMA user_version` stamped).
Default clips home is per-OS (`crate::os::paths`, so shared code never
branches): Linux = OS videos folder + `MoonClip` (`dirs::video_dir`);
Windows = `%LOCALAPPDATA%\MoonClip\Clips` (`dirs::data_local_dir`) —
deliberately outside Videos/Documents/Desktop, which sit under Controlled
Folder Access and are often redirected into OneDrive (an unsigned clip app
mass-writing `.mp4` there is blocked/flagged and syncs every clip to the
cloud). A one-time boot migration moves `*.mp4` + `thumb_*.jpg` from the
legacy `~/Videos/MoonClip` to the new home when the stored setting still
equals it (custom folders are never touched); DB rows need no migration
(relative names). DB at OS app-data dir. Changing `clips_directory` (C:→D:) needs zero row migration.

10k clips ≈ <5 MB.

Resolve physically:

```rust
pub fn resolve_clip_path(base_dir: &std::path::PathBuf, file_name: &str) -> std::path::PathBuf {
    base_dir.join(file_name)
}
```

Changing `clips_directory` (C:→D:) needs zero row migration.

## 3. Ghost-clip prevention

Users delete/move `.mp4` in Explorer/Nautilus. On `get_clips`, check `Path::exists()`; the UI marks the row as missing. Background
`reconcile_library` (startup + command) indexes files WITHOUT a row: OBS-style
names (`Replay …`, `*_trim*`, `*_edit*`) at the library root and any
`mp4`/`mkv` inside a game folder — `ffprobe` duration + thumbnail next to the
video, then the row. Orphan rows are removed only by the explicit
`purge_missing_clips` command (external drives can be offline). **Reconcile
never deletes files.**

## 4. Auto-pruning (LRU)

Setting `max_storage_gb` (0/empty = off, the default). Enforced after every
save/trim/export: the oldest non-favorite clips (file + thumbnail + row) are
removed until the library fits. The clip just saved and every favorite are
untouchable; if only those remain over quota nothing is deleted.

## 5. Secrets with `keyring`

```rust
use keyring::Entry;
pub fn store_drive_token(token: &str) -> Result<(), keyring::Error> {
    Entry::new("moonclip", "google_drive_refresh_token")?.set_password(token)
}
pub fn get_drive_token() -> Result<String, keyring::Error> {
    Entry::new("moonclip", "google_drive_refresh_token")?.get_password()
}
```

- Windows: Credential Manager (DPAPI).
- Linux: freedesktop Secret Service (GNOME Keyring / KWallet / KeePassXC).
- Minimal WMs (i3/Hyprland/Sway) may lack daemon → catch `NoStorage` DBus error, prompt "install/start gnome-keyring or kwallet". Optional fallback: AES-GCM (`aes-gcm` + Argon2 over `/etc/machine-id` + username) — document, implement only if needed.

## 6. Rust choice (decided Phase 2)

- `rusqlite` (backend-only, `tauri-plugin-sql` removed): expose
  `list_clips`/`toggle_favorite`/`delete_clip`/settings via `invoke()`.
  Never SQL from the frontend.

## 7. Per-game library layout (2026-09-27)

- Every registered game gets a folder: `<clips_dir>/<sanitized display_name>/`
  (Windows-safe: reserved names prefixed, invalid characters replaced, 80-char
  cap; unicode kept). An existing folder is REUSED case-insensitively, so
  re-registering a game merges its clips back into the same group.
- `clips.folder` + `custom_apps.clips_folder` (migration 013) keep the
  association **stable**: editing or deleting a registration never renames,
  moves or deletes the folder, its clips or its gallery group. `Unknown`
  holds clips captured with no game detected.
- OBS keeps writing at the library root; the save pipeline moves the file
  (and its thumbnail) into the game folder before indexing. Trim/export
  results stay in the source clip's folder.
- Legacy flat libraries organize themselves at boot (`organize_library`,
  idempotent): clips move to the folder of their `game_title`, missing files
  are left untouched. Relocating `clips_directory` carries game folders whole.
- Stored names are relative `<folder>/<file>` with exactly one level;
  `is_safe_relative_media_name` rejects traversal, absolute paths, drive
  prefixes, backslashes and dotted/empty segments everywhere a name is
  resolved or persisted.
