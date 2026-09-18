-- V3: embedded OBS engine preferences.
-- `obs_ws_password` is intentionally left empty: it is generated on first use
-- and persisted then (never committed, unique per install).
INSERT OR IGNORE INTO settings (key, value) VALUES
  ('video_encoder', 'gpu'),
  ('gpu_index', '0'),
  ('obs_ws_port', '4456'),
  ('obs_ws_password', '');
