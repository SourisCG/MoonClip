//! SQLite access (rusqlite, bundled). All queries run behind a Mutex.

use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::AppHandle;

use super::models::{ClipRecord, RegisteredInput};
use super::paths;

const SCHEMA_VERSION: i64 = 14;
const MIGRATION_001: &str = include_str!("../../migrations/001_init.sql");
const MIGRATION_002: &str = include_str!("../../migrations/002_gains.sql");
const MIGRATION_003: &str = include_str!("../../migrations/003_devices.sql");
const MIGRATION_004: &str = include_str!("../../migrations/004_video.sql");
const MIGRATION_005: &str = include_str!("../../migrations/005_fps.sql");
const MIGRATION_006: &str = include_str!("../../migrations/006_monitor.sql");
const MIGRATION_007: &str = include_str!("../../migrations/007_obs.sql");
const MIGRATION_008: &str = include_str!("../../migrations/008_custom_video.sql");
const MIGRATION_009: &str = include_str!("../../migrations/009_engine_keys.sql");
const MIGRATION_010: &str = include_str!("../../migrations/010_game_token.sql");
const MIGRATION_011: &str = include_str!("../../migrations/011_registered_inputs.sql");
const MIGRATION_012: &str = include_str!("../../migrations/012_window_identity.sql");
const MIGRATION_013: &str = include_str!("../../migrations/013_clip_folders.sql");
const MIGRATION_014: &str = include_str!("../../migrations/014_drive_folders.sql");

pub struct DbState(pub Mutex<Connection>);

/// In-memory DB with the full schema, for tests across modules.
#[cfg(test)]
pub(crate) fn test_db() -> DbState {
    let conn = Connection::open_in_memory().unwrap();
    for migration in [
        MIGRATION_001,
        MIGRATION_010,
        MIGRATION_011,
        MIGRATION_012,
        MIGRATION_013,
        MIGRATION_014,
    ] {
        conn.execute_batch(migration).unwrap();
    }
    DbState(Mutex::new(conn))
}

/// Add a column when it is missing (idempotent). Self-healing guard: a
/// version stamped by a partially-built binary must never leave the schema
/// behind (and costs one PRAGMA per open).
fn ensure_column(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<(), String> {
    let exists = {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|e| format!("cannot inspect {table}: {e}"))?;
        let cols = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| format!("cannot inspect {table}: {e}"))?
            .filter_map(Result::ok)
            .any(|c| c == column);
        cols
    };
    if !exists {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))
            .map_err(|e| format!("cannot add {table}.{column}: {e}"))?;
        eprintln!("[moonclip] schema repair: added {table}.{column}");
    }
    Ok(())
}

/// Older builds persisted the obs-websocket password and the portal restore
/// token in `settings`. Remove them on open (idempotent).
fn scrub_legacy_secret_settings(conn: &Connection) -> Result<usize, String> {
    conn.execute(
        "DELETE FROM settings WHERE key IN ('engine_ws_password', 'engine_restore_token')",
        [],
    )
    .map_err(|e| format!("cannot scrub legacy secrets: {e}"))
}

impl DbState {
    pub fn open(app: &AppHandle) -> Result<Self, String> {
        let db_path = paths::db_file_path(app)?;
        let mut conn =
            Connection::open(&db_path).map_err(|e| format!("cannot open database: {e}"))?;
        // Durability + integrity for an embedded single-process DB.
        let _: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .map_err(|e| format!("cannot enable WAL: {e}"))?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(|e| format!("cannot enable foreign keys: {e}"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| format!("cannot set busy timeout: {e}"))?;
        let _ = paths::harden_file(&db_path);
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| format!("cannot read schema version: {e}"))?;
        if version < SCHEMA_VERSION {
            // All pending migrations commit together: a failure half-way must
            // not leave a partially upgraded schema.
            let tx = conn
                .transaction()
                .map_err(|e| format!("cannot start migration transaction: {e}"))?;
            if version < 1 {
                tx.execute_batch(MIGRATION_001)
                    .map_err(|e| format!("migration 001 failed: {e}"))?;
            }
            if version < 2 {
                tx.execute_batch(MIGRATION_002)
                    .map_err(|e| format!("migration 002 failed: {e}"))?;
            }
            if version < 3 {
                tx.execute_batch(MIGRATION_003)
                    .map_err(|e| format!("migration 003 failed: {e}"))?;
            }
            if version < 4 {
                tx.execute_batch(MIGRATION_004)
                    .map_err(|e| format!("migration 004 failed: {e}"))?;
            }
            if version < 5 {
                tx.execute_batch(MIGRATION_005)
                    .map_err(|e| format!("migration 005 failed: {e}"))?;
            }
            if version < 6 {
                tx.execute_batch(MIGRATION_006)
                    .map_err(|e| format!("migration 006 failed: {e}"))?;
            }
            if version < 7 {
                tx.execute_batch(MIGRATION_007)
                    .map_err(|e| format!("migration 007 failed: {e}"))?;
            }
            if version < 8 {
                tx.execute_batch(MIGRATION_008)
                    .map_err(|e| format!("migration 008 failed: {e}"))?;
            }
            if version < 9 {
                tx.execute_batch(MIGRATION_009)
                    .map_err(|e| format!("migration 009 failed: {e}"))?;
            }
            if version < 10 {
                tx.execute_batch(MIGRATION_010)
                    .map_err(|e| format!("migration 010 failed: {e}"))?;
            }
            if version < 11 {
                tx.execute_batch(MIGRATION_011)
                    .map_err(|e| format!("migration 011 failed: {e}"))?;
            }
            if version < 12 {
                tx.execute_batch(MIGRATION_012)
                    .map_err(|e| format!("migration 012 failed: {e}"))?;
            }
            if version < 13 {
                tx.execute_batch(MIGRATION_013)
                    .map_err(|e| format!("migration 013 failed: {e}"))?;
            }
            if version < 14 {
                tx.execute_batch(MIGRATION_014)
                    .map_err(|e| format!("migration 014 failed: {e}"))?;
            }
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(|e| format!("cannot stamp schema version: {e}"))?;
            tx.commit()
                .map_err(|e| format!("cannot commit migrations: {e}"))?;
        }
        // Self-healing: guarantee the game-folder columns exist even if the
        // schema version was stamped by a partially-built binary.
        ensure_column(&conn, "clips", "folder", "TEXT NOT NULL DEFAULT ''")?;
        ensure_column(&conn, "custom_apps", "clips_folder", "TEXT NOT NULL DEFAULT ''")?;
        conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_clips_folder ON clips(folder)")
            .map_err(|e| format!("cannot index clips.folder: {e}"))?;
        conn.execute_batch(MIGRATION_014)
            .map_err(|e| format!("cannot ensure drive_folders: {e}"))?;
        // Secrets never live in the DB: drop rows written by older builds
        // (the websocket password is generated per start; the portal token
        // now lives in the OS vault).
        scrub_legacy_secret_settings(&conn)?;
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
                        file_size_bytes, created_at, is_favorite, drive_file_id, drive_web_url,
                        folder
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
                    folder: r.get(10)?,
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
        folder: &str,
    ) -> Result<ClipRecord, String> {
        if !paths::is_safe_relative_media_name(file_name)
            || !paths::is_safe_relative_media_name(thumbnail_name)
        {
            return Err("invalid file name".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO clips
             (id, file_name, thumbnail_name, game_title, duration_ms, file_size_bytes, folder)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                file_name,
                thumbnail_name,
                game_title,
                duration_ms,
                file_size_bytes,
                folder
            ],
        )
        .map_err(|e| format!("cannot insert clip: {e}"))?;
        let clip: ClipRecord = conn
            .query_row(
                "SELECT id, file_name, thumbnail_name, game_title, duration_ms,
                        file_size_bytes, created_at, is_favorite, drive_file_id, drive_web_url,
                        folder
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
                        folder: r.get(10)?,
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
            if !paths::is_safe_relative_media_name(name) {
                // A corrupt row must never make us delete outside the library.
                continue;
            }
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

    /// LRU pruning for `max_storage_gb` (docs/05): when the library exceeds
    /// the quota, delete the OLDEST non-favorite clips (file + thumbnail +
    /// row) until it fits. `protected_id` (the clip just saved) and favorites
    /// are never touched; if only those remain over quota, nothing is deleted.
    /// `quota_gb_override` exists for tests. Returns clips pruned.
    pub fn enforce_quota(
        &self,
        protected_id: Option<&str>,
        quota_gb_override: Option<f64>,
    ) -> Result<u32, String> {
        let quota_gb = match quota_gb_override {
            Some(v) => v,
            None => self
                .get_settings()?
                .get("max_storage_gb")
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(0.0),
        };
        if quota_gb <= 0.0 || quota_gb.is_nan() {
            return Ok(0);
        }
        let quota = (quota_gb * 1024.0 * 1024.0 * 1024.0) as i64;
        let base = self.clips_dir()?;
        let (mut total, candidates) = {
            let conn = self.lock()?;
            let total: i64 = conn
                .query_row(
                    "SELECT COALESCE(SUM(file_size_bytes), 0) FROM clips",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| format!("cannot sum clip sizes: {e}"))?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, file_name, thumbnail_name, file_size_bytes FROM clips
                     WHERE is_favorite = 0 AND id != COALESCE(?1, '')
                     ORDER BY created_at ASC, rowid ASC",
                )
                .map_err(|e| format!("cannot list prune candidates: {e}"))?;
            let rows = stmt
                .query_map(params![protected_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                })
                .map_err(|e| format!("cannot read prune candidates: {e}"))?;
            let candidates = rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("cannot read prune candidate row: {e}"))?;
            (total, candidates)
        };
        let mut pruned = 0u32;
        for (id, file_name, thumb_name, size) in candidates {
            if total <= quota {
                break;
            }
            for name in [&file_name, &thumb_name] {
                let path = paths::resolve_clip_path(&base, name);
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => eprintln!(
                        "[moonclip] prune: cannot delete {}: {e}",
                        path.display()
                    ),
                }
            }
            self.delete_row(&id)?;
            total -= size.max(0);
            pruned += 1;
            eprintln!("[moonclip] prune: removed {file_name} (quota {quota_gb} GB)");
        }
        Ok(pruned)
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
            "capture_mode",
            "engine_token_reset_v2",
            "engine_ws_port",
            "engine_source_width",
            "engine_source_height",
            "drive_root_folder_id",
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

    /// Registered games (window inputs). The screen input is internal and
    /// never listed here.
    pub fn list_registered_inputs(&self) -> Result<Vec<RegisteredInput>, String> {
        self.list_inputs_where("input_kind = 'window'")
    }

    /// Every input row, the internal screen one included (secret migration).
    pub fn all_registered_inputs(&self) -> Result<Vec<RegisteredInput>, String> {
        self.list_inputs_where("1 = 1")
    }

    /// The single internal full-screen input (created on first screen start).
    pub fn screen_input(&self) -> Result<Option<RegisteredInput>, String> {
        Ok(self.list_inputs_where("input_kind = 'screen'")?.into_iter().next())
    }

    fn list_inputs_where(&self, filter: &str) -> Result<Vec<RegisteredInput>, String> {
        let conn = self.lock()?;
        let sql = format!(
            "SELECT id, input_name, input_kind, display_name, window_title,
                    window_app_id, target_exe, input_settings, source_uuid, icon_path,
                    clips_folder
             FROM custom_apps
             WHERE input_name IS NOT NULL AND input_name != '' AND {filter}
             ORDER BY display_name COLLATE NOCASE"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("cannot list registered inputs: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(RegisteredInput {
                    id: r.get(0)?,
                    input_name: r.get(1)?,
                    input_kind: r.get(2)?,
                    display_name: r.get(3)?,
                    window_title: r.get(4)?,
                    window_app_id: r.get(5)?,
                    target_exe: r.get(6)?,
                    input_settings: r.get(7)?,
                    source_uuid: r.get(8)?,
                    icon_path: r.get(9)?,
                    clips_folder: r.get(10)?,
                })
            })
            .map_err(|e| format!("cannot list registered inputs: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("cannot read registered input row: {e}"))
    }

    pub fn input_by_name(&self, input_name: &str) -> Result<Option<RegisteredInput>, String> {
        Ok(self
            .list_inputs_where(&format!(
                "input_name = '{}'",
                input_name.replace('\'', "''")
            ))?
            .into_iter()
            .next())
    }

    /// Create the registry row for a new capture input.
    pub fn register_input(
        &self,
        input_name: &str,
        input_kind: &str,
        display_name: &str,
    ) -> Result<RegisteredInput, String> {
        if input_name.trim().is_empty() || display_name.trim().is_empty() {
            return Err("input_name and display_name are required".into());
        }
        if !matches!(input_kind, "window" | "screen") {
            return Err(format!("unknown input_kind: {input_kind}"));
        }
        let input = RegisteredInput {
            id: uuid::Uuid::new_v4().to_string(),
            input_name: input_name.to_string(),
            input_kind: input_kind.to_string(),
            display_name: display_name.to_string(),
            window_title: None,
            window_app_id: None,
            target_exe: String::new(),
            input_settings: None,
            source_uuid: uuid::Uuid::new_v4().to_string(),
            icon_path: None,
            clips_folder: String::new(),
        };
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO custom_apps
             (id, display_name, target_exe, match_strategy, clip_duration_seconds, icon_path,
              is_wine_proton, input_name, input_kind, input_settings, source_uuid)
             VALUES (?1, ?2, '', 'window', NULL, NULL, 0, ?3, ?4, NULL, ?5)",
            params![
                input.id,
                input.display_name,
                input.input_name,
                input.input_kind,
                input.source_uuid,
            ],
        )
        .map_err(|e| format!("cannot register input: {e}"))?;
        Ok(input)
    }

    /// Persist the OBS source settings (token/window target) for an input.
    pub fn set_input_settings(&self, input_name: &str, settings: &str) -> Result<(), String> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE custom_apps SET input_settings = ?1 WHERE input_name = ?2",
            params![settings, input_name],
        )
        .map_err(|e| format!("cannot store input settings: {e}"))?;
        Ok(())
    }

    /// Store what the picker taught us about the picked window.
    pub fn set_input_identity(
        &self,
        input_name: &str,
        display_name: &str,
        window_title: Option<&str>,
        window_app_id: Option<&str>,
        target_exe: &str,
    ) -> Result<(), String> {
        if display_name.trim().is_empty() {
            return Err("display_name is required".into());
        }
        let conn = self.lock()?;
        conn.execute(
            "UPDATE custom_apps
             SET display_name = ?1, window_title = ?2, window_app_id = ?3, target_exe = ?4
             WHERE input_name = ?5",
            params![display_name, window_title, window_app_id, target_exe, input_name],
        )
        .map_err(|e| format!("cannot store window identity: {e}"))?;
        Ok(())
    }

    /// Remember the Drive folder mirroring a local game folder.
    pub fn upsert_drive_folder(&self, folder: &str, drive_id: &str, name: &str) -> Result<(), String> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO drive_folders (folder, drive_id, name) VALUES (?1, ?2, ?3)
             ON CONFLICT(folder) DO UPDATE SET drive_id = excluded.drive_id, name = excluded.name",
            params![folder, drive_id, name],
        )
        .map_err(|e| format!("cannot store the drive folder: {e}"))?;
        Ok(())
    }

    pub fn drive_folder(&self, folder: &str) -> Result<Option<String>, String> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT drive_id FROM drive_folders WHERE folder = ?1",
            params![folder],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("cannot read the drive folder: {e}"))
    }

    /// Forget the Drive mirror (disconnect): the next connect re-creates it.
    pub fn clear_drive_folders(&self) -> Result<(), String> {
        let conn = self.lock()?;
        conn.execute("DELETE FROM drive_folders", [])
            .map_err(|e| format!("cannot clear drive_folders: {e}"))?;
        Ok(())
    }

    /// Library folder assigned to a game (created/reused at registration).
    pub fn set_input_folder(&self, input_name: &str, folder: &str) -> Result<(), String> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE custom_apps SET clips_folder = ?1 WHERE input_name = ?2",
            params![folder, input_name],
        )
        .map_err(|e| format!("cannot store game folder: {e}"))?;
        Ok(())
    }

    /// Move a clip to another folder (library organization) and update both
    /// relative names so the DB always matches the disk.
    pub fn set_clip_folder(
        &self,
        id: &str,
        folder: &str,
        file_name: &str,
        thumbnail_name: &str,
    ) -> Result<(), String> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE clips SET folder = ?1, file_name = ?2, thumbnail_name = ?3 WHERE id = ?4",
            params![folder, file_name, thumbnail_name, id],
        )
        .map_err(|e| format!("cannot move clip row: {e}"))?;
        Ok(())
    }

    /// Remove a registered input (the engine drops its OBS source too).
    pub fn delete_registered_input(&self, id: &str) -> Result<(), String> {
        let conn = self.lock()?;
        let changed = conn
            .execute(
                "DELETE FROM custom_apps WHERE id = ?1 AND input_name IS NOT NULL",
                params![id],
            )
            .map_err(|e| format!("cannot delete registered input: {e}"))?;
        if changed == 0 {
            return Err("registered input not found".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: removing a registered app must NEVER delete its clips,
    /// its folder association or anything on disk. Users uninstall games.
    #[test]
    fn deleting_a_registered_input_never_touches_its_clips() {
        let db = test_db();
        let input = db.register_input("Game", "window", "Juego").unwrap();
        db.set_input_folder("Game", "Juego").unwrap();
        let clip = db
            .insert_clip(
                "Juego/Replay 1.mp4",
                "Juego/thumb_Replay 1.jpg",
                "Juego",
                1000,
                10,
                "Juego",
            )
            .unwrap();
        db.delete_registered_input(&input.id).unwrap();
        assert!(db.list_registered_inputs().unwrap().is_empty());
        let clips = db.list_clips().unwrap();
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].id, clip.id);
        assert_eq!(clips[0].folder, "Juego");
        assert_eq!(clips[0].file_name, "Juego/Replay 1.mp4");
    }

    #[test]
    fn ensure_column_is_idempotent_and_repairs_partial_schemas() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE clips (id TEXT PRIMARY KEY);")
            .unwrap();
        ensure_column(&conn, "clips", "folder", "TEXT NOT NULL DEFAULT ''").unwrap();
        ensure_column(&conn, "clips", "folder", "TEXT NOT NULL DEFAULT ''").unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(clips)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(cols, vec!["id", "folder"]);
    }

    #[test]
    fn legacy_secret_settings_are_scrubbed_on_open() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings VALUES
               ('engine_ws_password','pw'),
               ('engine_restore_token','tok'),
               ('engine_ws_port','4456');",
        )
        .unwrap();
        assert_eq!(scrub_legacy_secret_settings(&conn).unwrap(), 2);
        let mut stmt = conn.prepare("SELECT key FROM settings").unwrap();
        let keys: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(keys, vec!["engine_ws_port"]);
    }

    #[test]
    fn migration_010_adds_portal_token() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATION_001).unwrap();
        conn.execute_batch(MIGRATION_010).unwrap();
        conn.execute_batch(MIGRATION_011).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(custom_apps)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(cols.iter().any(|c| c == "portal_token"));
    }

    fn tiny_db() -> DbState {
        test_db()
    }

    #[test]
    fn registered_input_round_trip() {
        let db = tiny_db();
        let input = db.register_input("Game", "window", "Juego 1").unwrap();
        assert_eq!(input.input_kind, "window");
        db.set_input_settings("Game", "{\"RestoreToken\":\"tok\"}")
            .unwrap();
        db.set_input_identity(
            "Game",
            "KINGDOM HEARTS FINAL MIX",
            Some("KINGDOM HEARTS FINAL MIX"),
            Some("steam_app_2552430"),
            "",
        )
        .unwrap();
        let listed = db.list_registered_inputs().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].display_name, "KINGDOM HEARTS FINAL MIX");
        assert_eq!(
            listed[0].window_title.as_deref(),
            Some("KINGDOM HEARTS FINAL MIX")
        );
        assert_eq!(listed[0].window_app_id.as_deref(), Some("steam_app_2552430"));
        assert_eq!(
            listed[0].input_settings.as_deref(),
            Some("{\"RestoreToken\":\"tok\"}")
        );
        // The screen input is internal: never listed as a game.
        db.register_input("MoonClip Screen", "screen", "Pantalla")
            .unwrap();
        assert_eq!(db.list_registered_inputs().unwrap().len(), 1);
        assert!(db.screen_input().unwrap().is_some());
        db.delete_registered_input(&input.id).unwrap();
        assert!(db.list_registered_inputs().unwrap().is_empty());
    }
    const MB: i64 = 1024 * 1024;

    fn quota_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "moonclip-quota-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn quota_prunes_oldest_non_favorites_only() {
        let db = tiny_db();
        let dir = quota_dir();
        db.set_setting("clips_directory", dir.to_str().unwrap())
            .unwrap();
        for name in [
            "old.mp4",
            "thumb_old.jpg",
            "fav.mp4",
            "thumb_fav.jpg",
            "new.mp4",
            "thumb_new.jpg",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let old = db
            .insert_clip("old.mp4", "thumb_old.jpg", "G", 1000, 900 * MB, "")
            .unwrap();
        let fav = db
            .insert_clip("fav.mp4", "thumb_fav.jpg", "G", 1000, 900 * MB, "")
            .unwrap();
        db.toggle_favorite(&fav.id).unwrap();
        let new = db
            .insert_clip("new.mp4", "thumb_new.jpg", "G", 1000, 900 * MB, "")
            .unwrap();
        // 2.7 GB total with a 1 GB quota: only the oldest non-favorite goes.
        assert_eq!(db.enforce_quota(Some(&new.id), Some(1.0)).unwrap(), 1);
        assert!(!dir.join("old.mp4").exists());
        assert!(!dir.join("thumb_old.jpg").exists());
        assert!(dir.join("fav.mp4").exists());
        assert!(dir.join("new.mp4").exists());
        let remaining: Vec<String> = db
            .list_clips()
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(remaining.len(), 2);
        assert!(!remaining.contains(&old.id));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quota_is_a_noop_without_a_limit() {
        let db = tiny_db();
        let dir = quota_dir();
        db.set_setting("clips_directory", dir.to_str().unwrap())
            .unwrap();
        let clip = db.insert_clip("a.mp4", "t.jpg", "G", 1000, 50 * MB, "").unwrap();
        assert_eq!(db.enforce_quota(Some(&clip.id), None).unwrap(), 0);
        assert_eq!(db.enforce_quota(Some(&clip.id), Some(0.0)).unwrap(), 0);
        assert_eq!(db.list_clips().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quota_never_prunes_favorites_or_the_new_clip() {
        let db = tiny_db();
        let dir = quota_dir();
        db.set_setting("clips_directory", dir.to_str().unwrap())
            .unwrap();
        let a = db.insert_clip("a.mp4", "t.jpg", "G", 1000, 2 * 1024 * MB, "").unwrap();
        db.toggle_favorite(&a.id).unwrap();
        let b = db.insert_clip("b.mp4", "t.jpg", "G", 1000, 2 * 1024 * MB, "").unwrap();
        // All over quota but every row is protected: nothing may be deleted.
        assert_eq!(db.enforce_quota(Some(&b.id), Some(1.0)).unwrap(), 0);
        assert_eq!(db.list_clips().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

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
}
