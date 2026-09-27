-- Google Drive mirror of the local game folders (Phase 6). One row per
-- local folder name (including 'Unknown'); the root folder id lives in
-- settings (drive_root_folder_id).
CREATE TABLE IF NOT EXISTS drive_folders (
  folder TEXT PRIMARY KEY,
  drive_id TEXT NOT NULL,
  name TEXT NOT NULL
);
