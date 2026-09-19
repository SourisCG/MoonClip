-- V3 Custom mode: full OBS video-output options (encoder schema + video tab).
-- Empty string = unused. Validated server-side (encoder_options registry),
-- never trusted raw.
INSERT OR IGNORE INTO settings (key, value) VALUES
  ('custom_encoder_json', ''),
  ('custom_video_json', '');
