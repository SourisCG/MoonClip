-- Drive-only clips: the video lives in Google Drive, the local file was
-- deleted after upload (optional, like Medal). The thumbnail stays local so
-- the gallery still renders. cloud = 1 marks the row.
-- (drive_file_id / drive_web_url already exist from the Phase 2 schema.)
ALTER TABLE clips ADD COLUMN cloud INTEGER NOT NULL DEFAULT 0;
