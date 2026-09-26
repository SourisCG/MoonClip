-- Registered capture inputs: one OBS source per app (name, kind, saved
-- settings such as the portal token or the Windows window target).
ALTER TABLE custom_apps ADD COLUMN input_name TEXT;
ALTER TABLE custom_apps ADD COLUMN input_kind TEXT;
ALTER TABLE custom_apps ADD COLUMN input_settings TEXT;
ALTER TABLE custom_apps ADD COLUMN source_uuid TEXT;
