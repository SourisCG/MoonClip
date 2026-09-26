-- V4: per-game state on custom_apps (window capture, portal token, prefs).
-- Rows are created by the user (registered once) or auto-created for known
-- games when state must be stored; `game_key` is the stable identity.
ALTER TABLE custom_apps ADD COLUMN game_key TEXT;
ALTER TABLE custom_apps ADD COLUMN capture_mode TEXT NOT NULL DEFAULT 'window';
ALTER TABLE custom_apps ADD COLUMN source_kind TEXT;
ALTER TABLE custom_apps ADD COLUMN window_match TEXT;
ALTER TABLE custom_apps ADD COLUMN portal_token TEXT;
ALTER TABLE custom_apps ADD COLUMN auto_buffer INTEGER NOT NULL DEFAULT 1;
ALTER TABLE custom_apps ADD COLUMN last_seen_ms INTEGER;
CREATE UNIQUE INDEX IF NOT EXISTS idx_custom_apps_game_key
  ON custom_apps(game_key) WHERE game_key IS NOT NULL;
