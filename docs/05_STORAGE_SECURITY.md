# 05 — Storage & Security (SQLite + OS keyring)

## 1. Rule: split data from secrets

| Type | Where | Tool | Security |
|---|---|---|---|
| Metadata (titles, dates, favorites, paths) | Local disk DB | SQLite (`rusqlite`, backend-only) | Plaintext, fast SQL |
| Secrets (OAuth tokens, webhook URLs, portal tokens) | OS vault | `keyring` crate | DPAPI/TPM/Win Credential Manager (Windows), Secret Service/KWallet (Linux) |
| TikTok client secret | Cloudflare Worker (`workers/moonclip-oauth/`) | `wrangler secret` | Never leaves Cloudflare; the app holds no secret |

SQLite is ~1 MB embedded, ~0 RAM idle. Do NOT use SQLCipher with a hardcoded
key (decompilable in OSS). Do NOT ask for a master password on every clip.

## 2. Secrets policy — NO SECRETS IN GIT (hard rule)

**The repository is public. Committing secrets is forbidden, no exceptions.**

- NEVER commit: `social.json`, OAuth client secrets, refresh/access tokens,
  Discord webhook URLs, TikTok client key/secret pairs, Cloudflare API tokens,
  private keys, `.dev.vars`, or screenshots that show any of them.
- Client credentials that ARE allowed in the repo: Google **Desktop** client ids
  (public by design) and placeholder values in `social.example.json`.
- The only place real secrets live:
  - **OS keyring** (tokens, Discord webhook).
  - **`social.json`** (Google/TikTok client credentials), gitignored and `0600`,
    outside the repository.
  - **Cloudflare Worker secrets** (TikTok client secret).
- If a secret is ever committed: rotate it first (Google console / Discord
  portal / TikTok portal / `wrangler secret put`), then rewrite history. A
  revert is NOT enough.
- `scripts/check-secrets.sh` scans the tree (and the staged diff) for known
  patterns (`GOCSPX`, `client_secret`, Discord webhook URLs, `sbaw*`/`rft.`/
  `act.` TikTok tokens, private-key headers) and fails on a hit. Run it before
  every push.

This rule is repeated on purpose in `README.md`, `SPEC.md`,
`09_WINDOWS_HANDOFF.md`, `11_AUDIO.md` and `social.example.json`.

## 3. Config outside the repo: `social.json`

Public provider credentials live in the app-data directory (env vars win over
the file; see `social.example.json`):

- Linux: `~/.local/share/dev.souriscg.moonclip/social.json`
- Windows: `%APPDATA%\dev.souriscg.moonclip\social.json`

```json
{
  "google_drive":   { "client_id": "….apps.googleusercontent.com", "client_secret": "GOCSPX-…" },
  "google_youtube": { "client_id": "….apps.googleusercontent.com", "client_secret": "GOCSPX-…" },
  "discord":        { "client_id": "OPTIONAL: MoonClip's public app is the default" },
  "tiktok":         { "client_id": "TikTok client key", "client_secret": "" },
  "tiktok_worker_url": "https://moonclip-oauth.<subdomain>.workers.dev"
}
```

- The file is `0600`, gitignored (`**/social.json`) and never crosses IPC.
- Discord needs no entry (public client id is embedded); TikTok's secret is
  empty here because the Worker adds it during exchange/refresh.
- Env overrides: `MOONCLIP_GOOGLE_DRIVE_CLIENT_ID`, `MOONCLIP_GOOGLE_DRIVE_CLIENT_SECRET`,
  `MOONCLIP_DISCORD_CLIENT_ID`, `MOONCLIP_TIKTOK_WORKER_URL`.

## 4. Schema (relative paths only)

```sql
CREATE TABLE clips (
  id TEXT PRIMARY KEY,
  file_name TEXT NOT NULL UNIQUE,       -- '<folder>/<file>.mp4' (relative, one level)
  thumbnail_name TEXT NOT NULL,
  game_title TEXT NOT NULL,
  duration_ms INTEGER NOT NULL,
  file_size_bytes INTEGER NOT NULL,     -- LRU quota
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  is_favorite INTEGER DEFAULT 0,        -- protected from auto-delete
  drive_file_id TEXT,                   -- set on EVERY Drive upload
  drive_web_url TEXT,                   -- set when made public
  folder TEXT NOT NULL DEFAULT '',      -- migration 013: stable game folder
  cloud INTEGER NOT NULL DEFAULT 0      -- migration 015: video lives in Drive
);

CREATE TABLE custom_apps (              -- registered capture inputs
  id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  target_exe TEXT NOT NULL,             -- legacy, unused
  match_strategy TEXT NOT NULL,         -- legacy, unused
  clip_duration_seconds INTEGER,
  icon_path TEXT,
  is_wine_proton INTEGER DEFAULT 0,     -- legacy, unused
  input_name TEXT, input_kind TEXT, input_settings TEXT, source_uuid TEXT,  -- 011
  window_title TEXT, window_app_id TEXT,                                    -- 012
  clips_folder TEXT NOT NULL DEFAULT ''                                     -- 013
);

CREATE TABLE drive_folders (            -- 014: Drive mirror of game folders
  folder TEXT PRIMARY KEY,
  drive_id TEXT NOT NULL,
  name TEXT NOT NULL
);

CREATE TABLE settings ( key TEXT PRIMARY KEY, value TEXT NOT NULL );
```

Migrations live in `src-tauri/migrations/` (001…015, `PRAGMA user_version`
stamped, one transaction). Settings keys (non-exhaustive): `clips_directory`,
`buffer_seconds`, `hotkey`, `max_storage_gb`, `locale`, `gain_game`/`gain_mic`
(0–100), `mute_game`/`mute_mic`, `mic_device`/`desktop_device`,
`audio_single_track`, `video_quality`, `video_codec`/`video_encoder`,
`custom_encoder_json`/`custom_video_json`, `fps`, `monitor`, `container`,
`discord_max_mb`, `drive_root_folder_id`, `engine_ws_port`, `setup_done`,
`last_game_input`, `share_delete_local`.

## 5. Paths

- **Clips home** is per-OS (`crate::os::paths`):
  - Linux: the OS videos folder + `MoonClip` (`dirs::video_dir`).
  - Windows: `%LOCALAPPDATA%\MoonClip\Clips` (`dirs::data_local_dir`) —
    deliberately outside Videos/Documents/Desktop (Controlled Folder Access /
    OneDrive redirection would block or sync every clip).
  A one-time boot migration moves `*.mp4` + `thumb_*.jpg` from the legacy
  `~/Videos/MoonClip` when the stored setting still equals it; custom folders
  are never touched. Changing `clips_directory` needs zero row migration.
- **DB + app data** live in the OS app-data dir
  (`~/.local/share/dev.souriscg.moonclip` / `%APPDATA%\dev.souriscg.moonclip`),
  with `moonclip.db` + WAL/SHM at `0600` and the app-data dir at `0700`.
- Names are relative `<folder>/<file>` with exactly one level;
  `is_safe_relative_media_name` rejects traversal, absolute paths, drive
  prefixes, backslashes and dotted/empty segments everywhere a name is resolved
  or persisted.

## 6. Keyring entries

Service `moonclip`, account = alias:

| Alias | Contents |
|---|---|
| `oauth_google_drive` | Drive token blob (access + refresh + expiry + email) |
| `oauth_google_youtube` | YouTube token blob (`youtube.upload` scope) |
| `oauth_tiktok` | TikTok token blob (rotating refresh token) |
| `discord_webhook` | Captured webhook (id, token, url, guild/channel, label) |
| `portal_<input_id>` | XDG portal `RestoreToken` for a registered input |

- Windows: Credential Manager (DPAPI). Linux: Secret Service/KWallet.
- Minimal WMs without a daemon surface a clear error; MoonClip never writes
  secrets to the DB as a fallback (the legacy portal-token fallback predates
  this rule and is removed by the vault migration).
- The **obs-websocket password is ephemeral**: generated per engine start,
  written only into that run's generated OBS config, never persisted.

## 7. Per-game library layout

- Every registered game gets a folder: `<clips_dir>/<sanitized display_name>/`
  (Windows-safe: reserved names prefixed, invalid characters replaced, 80-char
  cap; unicode kept). Existing folders are reused case-insensitively.
- `clips.folder` + `custom_apps.clips_folder` keep the association **stable**:
  editing or deleting a registration never renames, moves or deletes the folder,
  its clips or its gallery group. `Unknown` holds clips captured with no game
  detected.
- OBS keeps writing at the library root; the save pipeline moves the file (and
  its thumbnail) into the game folder before indexing. Trim/export results stay
  in the source clip's folder.
- Legacy flat libraries organize themselves at boot (`organize_library`,
  idempotent); missing files are left untouched.

## 8. Ghost clips, quota and cloud clips

- Users delete/move `.mp4` files. `list_clips` computes `exists`; the UI marks
  missing rows. `reconcile_library` (startup + command) indexes files without a
  row (OBS-style names, `*_trim*`/`*_edit*`, any `mp4`/`mkv` inside a game
  folder) with duration + thumbnail; **it never deletes files**. Orphan rows are
  removed only by `purge_missing_clips`.
- `max_storage_gb` (0 = off): after every save/trim/export the oldest
  non-favorite clips are removed until the library fits; the clip just saved and
  every favorite are untouchable. Rows with `cloud = 1` are skipped (their video
  is not on disk).
- **Cloud clips** (Drive-only): `cloud = 1` + `drive_file_id`; the thumbnail
  stays local so the gallery renders. Playback/trim use `ensure_local` (download
  to the app cache with progress); deleting a cloud clip sends the remote file to
  the Drive trash. `drive_sync_library` restores them on another machine
  (`06_SOCIAL_INTEGRATIONS.md`).

## 9. Rust choice

`rusqlite` (backend-only): expose `list_clips`/`toggle_favorite`/`delete_clip`/
settings through `invoke()`. Never SQL from the frontend.

## 10. Reminder

Again: **no secrets in git** (§2). If you are a contributor or an AI agent
working on this repository, assume everything you write is public forever.
