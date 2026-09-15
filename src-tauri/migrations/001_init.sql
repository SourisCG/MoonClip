-- MoonClip initial schema (Phase 2).
-- RULE: file_name / thumbnail_name are RELATIVE to the configured clips directory.
-- Never store absolute paths here.

CREATE TABLE IF NOT EXISTS clips (
  id TEXT PRIMARY KEY,
  file_name TEXT NOT NULL UNIQUE,
  thumbnail_name TEXT NOT NULL,
  game_title TEXT NOT NULL,
  duration_ms INTEGER NOT NULL,
  file_size_bytes INTEGER NOT NULL,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  is_favorite INTEGER NOT NULL DEFAULT 0,
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
  is_wine_proton INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT OR IGNORE INTO settings (key, value) VALUES
  ('clips_directory', ''),
  ('buffer_seconds', '30'),
  ('hotkey', 'F9'),
  ('max_storage_gb', '20'),
  ('locale', 'es');
