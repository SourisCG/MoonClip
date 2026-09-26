-- Per-game portal window token: restored on the next start so the picker
-- appears once per registered game.
ALTER TABLE custom_apps ADD COLUMN portal_token TEXT;
