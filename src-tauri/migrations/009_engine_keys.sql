-- V3.6: rename engine settings keys so the upstream product name never
-- appears in the local database. Idempotent: UPDATE OR IGNORE preserves an
-- existing target row, then the old key is dropped.
UPDATE OR IGNORE settings SET key = 'engine_ws_port' WHERE key = 'obs_ws_port';
DELETE FROM settings WHERE key = 'obs_ws_port';

UPDATE OR IGNORE settings SET key = 'engine_ws_password' WHERE key = 'obs_ws_password';
DELETE FROM settings WHERE key = 'obs_ws_password';

UPDATE OR IGNORE settings SET key = 'engine_restore_token' WHERE key = 'obs_restore_token';
DELETE FROM settings WHERE key = 'obs_restore_token';

UPDATE OR IGNORE settings SET key = 'engine_source_width' WHERE key = 'obs_source_width';
DELETE FROM settings WHERE key = 'obs_source_width';

UPDATE OR IGNORE settings SET key = 'engine_source_height' WHERE key = 'obs_source_height';
DELETE FROM settings WHERE key = 'obs_source_height';
