-- Per-game library organization. Clips live in `<clips_dir>/<folder>/` and the
-- association is stored so it survives app edits/removals: deleting a
-- registration must never orphan or hide its clips.
ALTER TABLE clips ADD COLUMN folder TEXT NOT NULL DEFAULT '';
ALTER TABLE custom_apps ADD COLUMN clips_folder TEXT NOT NULL DEFAULT '';
CREATE INDEX IF NOT EXISTS idx_clips_folder ON clips(folder);
