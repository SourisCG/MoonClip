//! SQLite access (rusqlite, bundled). All queries run behind a Mutex.

use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::AppHandle;

use super::models::{ClipRecord, CustomApp, RegisterAppInput};
use super::paths;

const SCHEMA_VERSION: i64 = 10;
const MIGRATION_001: &str = include_str!("../../migrations/001_init.sql");
const MIGRATION_002: &str = include_str!("../../migrations/002_gains.sql");
const MIGRATION_003: &str = include_str!("../../migrations/003_devices.sql");
const MIGRATION_004: &str = include_str!("../../migrations/004_video.sql");
const MIGRATION_005: &str = include_str!("../../migrations/005_fps.sql");
const MIGRATION_006: &str = include_str!("../../migrations/006_monitor.sql");
const MIGRATION_007: &str = include_str!("../../migrations/007_obs.sql");
const MIGRATION_008: &str = include_str!("../../migrations/008_custom_video.sql");
const MIGRATION_009: &str = include_str!("../../migrations/009_engine_keys.sql");
const MIGRATION_010: &str = include_str!("../../migrations/010_game_state.sql");

pub struct DbState(pub Mutex<Connection>);

impl DbState {
    pub fn open(app: &AppHandle) -> Result<Self, String> {
        let db_path = paths::db_file_path(app)?;
        let conn = Connection::open(&db_path).map_err(|e| format!("cannot open database: {e}"))?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| format!("cannot read schema version: {e}"))?;
        if version < SCHEMA_VERSION {
            if version < 1 {
                conn.execute_batch(MIGRATION_001)
                    .map_err(|e| format!("migration 001 failed: {e}"))?;
            }
            if version < 2 {
                conn.execute_batch(MIGRATION_002)
                    .map_err(|e| format!("migration 002 failed: {e}"))?;
            }
            if version < 3 {
                conn.execute_batch(MIGRATION_003)
                    .map_err(|e| format!("migration 003 failed: {e}"))?;
            }
            if version < 4 {
                conn.execute_batch(MIGRATION_004)
                    .map_err(|e| format!("migration 004 failed: {e}"))?;
            }
            if version < 5 {
                conn.execute_batch(MIGRATION_005)
                    .map_err(|e| format!("migration 005 failed: {e}"))?;
            }
            if version < 6 {
                conn.execute_batch(MIGRATION_006)
                    .map_err(|e| format!("migration 006 failed: {e}"))?;
            }
            if version < 7 {
                conn.execute_batch(MIGRATION_007)
                    .map_err(|e| format!("migration 007 failed: {e}"))?;
            }
            if version < 8 {
                conn.execute_batch(MIGRATION_008)
                    .map_err(|e| format!("migration 008 failed: {e}"))?;
            }
            if version < 9 {
                conn.execute_batch(MIGRATION_009)
                    .map_err(|e| format!("migration 009 failed: {e}"))?;
            }
            if version < 10 {
                conn.execute_batch(MIGRATION_010)
                    .map_err(|e| format!("migration 010 failed: {e}"))?;
            }
            conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
                .map_err(|e| format!("cannot stamp schema version: {e}"))?;
        }
        let state = Self(Mutex::new(conn));
        state.ensure_clips_dir()?;
        Ok(state)
    }

    /// Fill empty `clips_directory` setting with the platform default and create it.
    /// One-time relocation: when the stored dir is still the legacy Windows
    /// default (~/Videos/MoonClip, pre AV-safe home), move our files to the
    /// new home and repoint the setting. DB rows are untouched (relative).
    /// Custom user folders are never migrated.
    fn ensure_clips_dir(&self) -> Result<(), String> {
        let conn = self.lock()?;
        let current: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'clips_directory'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("cannot read clips_directory: {e}"))?
            .unwrap_or_default();
        if current.trim().is_empty() {
            let dir = paths::default_clips_dir();
            std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create clips dir: {e}"))?;
            conn.execute(
                "UPDATE settings SET value = ?1 WHERE key = 'clips_directory'",
                params![dir.to_string_lossy()],
            )
            .map_err(|e| format!("cannot save clips_directory: {e}"))?;
        } else if let Some(legacy) = crate::os::paths::legacy_default_clips_dir() {
            let fresh = paths::default_clips_dir();
            if Path::new(current.trim()) == legacy.as_path() && legacy != fresh && legacy.is_dir() {
                let moved = paths::migrate_legacy_clips_dir(&legacy, &fresh)?;
                conn.execute(
                    "UPDATE settings SET value = ?1 WHERE key = 'clips_directory'",
                    params![fresh.to_string_lossy()],
                )
                .map_err(|e| format!("cannot save clips_directory: {e}"))?;
                eprintln!(
                    "[moonclip] clips library relocated ({moved} files): {} -> {}",
                    legacy.display(),
                    fresh.display()
                );
                return Ok(());
            }
            std::fs::create_dir_all(&current)
                .map_err(|e| format!("cannot create clips dir: {e}"))?;
        } else {
            std::fs::create_dir_all(&current)
                .map_err(|e| format!("cannot create clips dir: {e}"))?;
        }
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        self.0
            .lock()
            .map_err(|e| format!("database lock poisoned: {e}"))
    }

    pub fn clips_dir(&self) -> Result<PathBuf, String> {
        let conn = self.lock()?;
        let dir: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'clips_directory'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| format!("clips_directory not set: {e}"))?;
        Ok(PathBuf::from(dir))
    }

    pub fn list_clips(&self) -> Result<Vec<ClipRecord>, String> {
        let conn = self.lock()?;
        let base = PathBuf::from(
            conn.query_row(
                "SELECT value FROM settings WHERE key = 'clips_directory'",
                [],
                |r| r.get::<_, String>(0),
            )
            .map_err(|e| format!("clips_directory not set: {e}"))?,
        );
        let mut stmt = conn
            .prepare(
                "SELECT id, file_name, thumbnail_name, game_title, duration_ms,
                        file_size_bytes, created_at, is_favorite, drive_file_id, drive_web_url
                 FROM clips ORDER BY created_at DESC",
            )
            .map_err(|e| format!("cannot prepare clips query: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ClipRecord {
                    id: r.get(0)?,
                    file_name: r.get(1)?,
                    thumbnail_name: r.get(2)?,
                    game_title: r.get(3)?,
                    duration_ms: r.get(4)?,
                    file_size_bytes: r.get(5)?,
                    created_at: r.get(6)?,
                    is_favorite: r.get::<_, i64>(7)? != 0,
                    drive_file_id: r.get(8)?,
                    drive_web_url: r.get(9)?,
                    exists: false, // filled below
                })
            })
            .map_err(|e| format!("cannot list clips: {e}"))?;
        let mut clips = Vec::new();
        for row in rows {
            let mut clip = row.map_err(|e| format!("cannot read clip row: {e}"))?;
            clip.exists = paths::resolve_clip_path(&base, &clip.file_name).exists();
            clips.push(clip);
        }
        Ok(clips)
    }

    /// Insert a freshly saved clip. Names are RELATIVE to the clips dir.
    pub fn insert_clip(
        &self,
        file_name: &str,
        thumbnail_name: &str,
        game_title: &str,
        duration_ms: i64,
        file_size_bytes: i64,
    ) -> Result<ClipRecord, String> {
        if file_name.contains("..") || file_name.starts_with('/') {
            return Err("invalid file name".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO clips
             (id, file_name, thumbnail_name, game_title, duration_ms, file_size_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                file_name,
                thumbnail_name,
                game_title,
                duration_ms,
                file_size_bytes
            ],
        )
        .map_err(|e| format!("cannot insert clip: {e}"))?;
        let clip: ClipRecord = conn
            .query_row(
                "SELECT id, file_name, thumbnail_name, game_title, duration_ms,
                        file_size_bytes, created_at, is_favorite, drive_file_id, drive_web_url
                 FROM clips WHERE id = ?1",
                params![id],
                |r| {
                    Ok(ClipRecord {
                        id: r.get(0)?,
                        file_name: r.get(1)?,
                        thumbnail_name: r.get(2)?,
                        game_title: r.get(3)?,
                        duration_ms: r.get(4)?,
                        file_size_bytes: r.get(5)?,
                        created_at: r.get(6)?,
                        is_favorite: r.get::<_, i64>(7)? != 0,
                        drive_file_id: r.get(8)?,
                        drive_web_url: r.get(9)?,
                        exists: true,
                    })
                },
            )
            .map_err(|e| format!("cannot read inserted clip: {e}"))?;
        Ok(clip)
    }

    /// Correct a stored duration (measured, not assumed).
    pub fn update_duration(&self, id: &str, duration_ms: i64) -> Result<(), String> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE clips SET duration_ms = ?1 WHERE id = ?2",
            params![duration_ms, id],
        )
        .map_err(|e| format!("cannot update duration: {e}"))?;
        Ok(())
    }

    pub fn toggle_favorite(&self, id: &str) -> Result<bool, String> {
        let conn = self.lock()?;
        let changed = conn
            .execute(
                "UPDATE clips SET is_favorite = 1 - is_favorite WHERE id = ?1",
                params![id],
            )
            .map_err(|e| format!("cannot toggle favorite: {e}"))?;
        if changed == 0 {
            return Err("clip not found".into());
        }
        conn.query_row(
            "SELECT is_favorite FROM clips WHERE id = ?1",
            params![id],
            |r| r.get::<_, i64>(0),
        )
        .map(|v| v != 0)
        .map_err(|e| format!("cannot read favorite state: {e}"))
    }

    /// Delete the DB row and its files (clip + thumbnail) when present.
    pub fn delete_clip(&self, id: &str) -> Result<(), String> {
        let conn = self.lock()?;
        let (file_name, thumb_name): (String, String) = conn
            .query_row(
                "SELECT file_name, thumbnail_name FROM clips WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("cannot find clip: {e}"))?
            .ok_or_else(|| "clip not found".to_string())?;
        let base: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'clips_directory'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| format!("clips_directory not set: {e}"))?;
        let base = PathBuf::from(base);
        for name in [&file_name, &thumb_name] {
            let p = paths::resolve_clip_path(&base, name);
            if p.exists() {
                std::fs::remove_file(&p)
                    .map_err(|e| format!("cannot delete file {}: {e}", p.display()))?;
            }
        }
        conn.execute("DELETE FROM clips WHERE id = ?1", params![id])
            .map_err(|e| format!("cannot delete clip row: {e}"))?;
        Ok(())
    }

    /// Delete only the DB row (files already gone). Used by purge-missing.
    pub fn delete_row(&self, id: &str) -> Result<(), String> {
        let conn = self.lock()?;
        conn.execute("DELETE FROM clips WHERE id = ?1", params![id])
            .map_err(|e| format!("cannot delete clip row: {e}"))?;
        Ok(())
    }

    pub fn get_settings(&self) -> Result<HashMap<String, String>, String> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare("SELECT key, value FROM settings")
            .map_err(|e| format!("cannot read settings: {e}"))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| format!("cannot read settings: {e}"))?;
        let mut map = HashMap::new();
        for row in rows {
            let (k, v) = row.map_err(|e| format!("cannot read setting row: {e}"))?;
            map.insert(k, v);
        }
        Ok(map)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        const ALLOWED: &[&str] = &[
            "clips_directory",
            "buffer_seconds",
            "hotkey",
            "max_storage_gb",
            "locale",
            "gain_game",
            "gain_mic",
            "mute_game",
            "mute_mic",
            "audio_single_track",
            "mic_device",
            "desktop_device",
            "video_codec",
            "video_encoder",
            "gpu_index",
            "out_height",
            "fps",
            "monitor",
            "container",
            "video_mode",
            "custom_bitrate_kbps",
            "custom_fps",
            "custom_encoder_json",
            "custom_video_json",
            "setup_done",
            "capture_max_fps",
            "faststart",
            "capture_window",
            "engine_ws_port",
            "engine_ws_password",
            "engine_restore_token",
            "engine_source_width",
            "engine_source_height",
        ];
        if !ALLOWED.contains(&key) {
            return Err(format!("unknown setting: {key}"));
        }
        match key {
            "container" if !matches!(value, "mp4" | "mkv") => {
                return Err("container must be mp4 or mkv".into());
            }
            "video_mode" if !matches!(value, "ladder" | "custom") => {
                return Err("video_mode must be ladder or custom".into());
            }
            "video_encoder" if !matches!(value, "gpu" | "cpu") => {
                return Err("video_encoder must be gpu or cpu".into());
            }
            "gpu_index" => {
                value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| "gpu_index must be a number".to_string())?;
            }
            "setup_done" if !matches!(value, "0" | "1") => {
                return Err("setup_done must be 0 or 1".into());
            }
            "faststart" if !matches!(value, "0" | "1") => {
                return Err("faststart must be 0 or 1".into());
            }
            "capture_max_fps" => {
                let v: u32 = value
                    .trim()
                    .parse()
                    .map_err(|_| "capture_max_fps must be a number".to_string())?;
                if v != 0 && !(30..=1000).contains(&v) {
                    return Err("capture_max_fps must be 0 (auto) or 30-1000".into());
                }
            }
            _ => {}
        }
        if key == "clips_directory" {
            let dir = PathBuf::from(value);
            std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create clips dir: {e}"))?;
        }
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(|e| format!("cannot save setting: {e}"))?;
        Ok(())
    }

    pub fn list_custom_apps(&self) -> Result<Vec<CustomApp>, String> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, display_name, target_exe, match_strategy,
                        clip_duration_seconds, icon_path, is_wine_proton,
                        game_key, capture_mode, source_kind, window_match,
                        portal_token, auto_buffer, last_seen_ms
                 FROM custom_apps ORDER BY display_name",
            )
            .map_err(|e| format!("cannot list custom apps: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(CustomApp {
                    id: r.get(0)?,
                    display_name: r.get(1)?,
                    target_exe: r.get(2)?,
                    match_strategy: r.get(3)?,
                    clip_duration_seconds: r.get(4)?,
                    icon_path: r.get(5)?,
                    is_wine_proton: r.get::<_, i64>(6)? != 0,
                    game_key: r.get(7)?,
                    capture_mode: r.get(8)?,
                    source_kind: r.get(9)?,
                    window_match: r.get(10)?,
                    portal_token: r.get(11)?,
                    auto_buffer: r.get::<_, i64>(12)? != 0,
                    last_seen_ms: r.get(13)?,
                })
            })
            .map_err(|e| format!("cannot list custom apps: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("cannot read custom app row: {e}"))
    }

    pub fn register_app(&self, input: RegisterAppInput) -> Result<CustomApp, String> {
        const STRATEGIES: &[&str] = &[
            "exact_exe",
            "cmdline_contains",
            "window_title",
            "wine_target",
            "steam_appid",
            "prism_instance",
        ];
        if input.display_name.trim().is_empty() || input.target_exe.trim().is_empty() {
            return Err("display_name and target_exe are required".into());
        }
        if !STRATEGIES.contains(&input.match_strategy.as_str()) {
            return Err(format!("unknown match_strategy: {}", input.match_strategy));
        }
        let app = CustomApp {
            id: uuid::Uuid::new_v4().to_string(),
            display_name: input.display_name,
            target_exe: input.target_exe,
            match_strategy: input.match_strategy,
            clip_duration_seconds: input.clip_duration_seconds,
            icon_path: None,
            is_wine_proton: input.is_wine_proton.unwrap_or(false),
            game_key: input.game_key,
            capture_mode: "window".to_string(),
            source_kind: input.source_kind,
            window_match: input.window_match,
            portal_token: None,
            auto_buffer: input.auto_buffer.unwrap_or(true),
            last_seen_ms: None,
        };
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO custom_apps
             (id, display_name, target_exe, match_strategy, clip_duration_seconds, icon_path,
              is_wine_proton, game_key, capture_mode, source_kind, window_match, portal_token,
              auto_buffer, last_seen_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                app.id,
                app.display_name,
                app.target_exe,
                app.match_strategy,
                app.clip_duration_seconds,
                app.icon_path,
                if app.is_wine_proton { 1 } else { 0 },
                app.game_key,
                app.capture_mode,
                app.source_kind,
                app.window_match,
                app.portal_token,
                if app.auto_buffer { 1 } else { 0 },
                app.last_seen_ms,
            ],
        )
        .map_err(|e| format!("cannot register app: {e}"))?;
        Ok(app)
    }

    /// Create or refresh the per-game row and persist capture state. User
    /// prefs (duration, auto_buffer, icon) are never overwritten; a `None`
    /// token/window keeps the stored one.
    pub fn set_game_capture(
        &self,
        game_key: &str,
        display_name: &str,
        source_kind: Option<&str>,
        window_match: Option<&str>,
        token: Option<&str>,
        now_ms: i64,
    ) -> Result<(), String> {
        if game_key.trim().is_empty() {
            return Err("empty game_key".into());
        }
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO custom_apps
             (id, display_name, target_exe, match_strategy, clip_duration_seconds, icon_path,
              is_wine_proton, game_key, capture_mode, source_kind, window_match, portal_token,
              auto_buffer, last_seen_ms)
             VALUES (?1, ?2, '', 'auto', NULL, NULL, 0, ?3, 'window', ?4, ?5, ?6, 1, ?7)
             ON CONFLICT(game_key) WHERE game_key IS NOT NULL DO UPDATE SET
               display_name = excluded.display_name,
               source_kind  = COALESCE(excluded.source_kind, source_kind),
               window_match = COALESCE(excluded.window_match, window_match),
               portal_token = COALESCE(excluded.portal_token, portal_token),
               last_seen_ms = excluded.last_seen_ms",
            params![
                uuid::Uuid::new_v4().to_string(),
                display_name,
                game_key,
                source_kind,
                window_match,
                token,
                now_ms,
            ],
        )
        .map_err(|e| format!("cannot store game capture state: {e}"))?;
        Ok(())
    }

    pub fn delete_app(&self, id: &str) -> Result<(), String> {
        let conn = self.lock()?;
        let changed = conn
            .execute("DELETE FROM custom_apps WHERE id = ?1", params![id])
            .map_err(|e| format!("cannot delete app: {e}"))?;
        if changed == 0 {
            return Err("app not found".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_009_renames_engine_keys() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings VALUES
               ('obs_ws_port','4456'),
               ('obs_restore_token','tok'),
               ('engine_source_width','1920');",
        )
        .unwrap();
        conn.execute_batch(MIGRATION_009).unwrap();
        let mut stmt = conn
            .prepare("SELECT key FROM settings ORDER BY key")
            .unwrap();
        let keys: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            keys,
            vec![
                "engine_restore_token",
                "engine_source_width",
                "engine_ws_port"
            ]
        );
        let port: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'engine_ws_port'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(port, "4456");
    }

    fn game_state_db() -> DbState {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATION_001).unwrap();
        conn.execute_batch(MIGRATION_010).unwrap();
        DbState(Mutex::new(conn))
    }

    #[test]
    fn migration_010_adds_game_state_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATION_001).unwrap();
        conn.execute_batch(MIGRATION_010).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(custom_apps)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for expected in [
            "game_key",
            "capture_mode",
            "source_kind",
            "window_match",
            "portal_token",
            "auto_buffer",
            "last_seen_ms",
        ] {
            assert!(cols.iter().any(|c| c == expected), "missing {expected}");
        }
    }

    #[test]
    fn game_capture_state_is_upserted_and_preserves_prefs() {
        let db = game_state_db();
        db.set_game_capture(
            "steam:570",
            "Dota 2",
            Some("x11"),
            Some("1\r\nD\r\ndota"),
            Some("tok-1"),
            111,
        )
        .unwrap();
        // Second call without token/window: keep both, update the name.
        db.set_game_capture("steam:570", "Dota 2 v2", None, None, None, 222)
            .unwrap();
        let apps = db.list_custom_apps().unwrap();
        assert_eq!(apps.len(), 1);
        let app = &apps[0];
        assert_eq!(app.display_name, "Dota 2 v2");
        assert_eq!(app.game_key.as_deref(), Some("steam:570"));
        assert_eq!(app.portal_token.as_deref(), Some("tok-1"));
        assert_eq!(app.window_match.as_deref(), Some("1\r\nD\r\ndota"));
        assert_eq!(app.last_seen_ms, Some(222));
        assert_eq!(app.match_strategy, "auto");
    }

    #[test]
    fn register_app_persists_game_state_fields() {
        let db = game_state_db();
        let app = db
            .register_app(RegisterAppInput {
                display_name: "Mi Juego".into(),
                target_exe: "mijuego.exe".into(),
                match_strategy: "wine_target".into(),
                clip_duration_seconds: Some(45),
                is_wine_proton: Some(true),
                game_key: Some("wine:mijuego.exe".into()),
                source_kind: Some("x11".into()),
                window_match: Some("7\r\nMi Juego\r\nmijuego".into()),
                auto_buffer: Some(false),
            })
            .unwrap();
        assert_eq!(app.game_key.as_deref(), Some("wine:mijuego.exe"));
        assert!(!app.auto_buffer);
        assert_eq!(app.source_kind.as_deref(), Some("x11"));
        let listed = db.list_custom_apps().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].clip_duration_seconds, Some(45));
        assert!(listed[0].is_wine_proton);
        db.delete_app(&app.id).unwrap();
        assert!(db.list_custom_apps().unwrap().is_empty());
    }
}
