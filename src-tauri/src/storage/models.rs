use serde::{Deserialize, Serialize};

/// One row of `clips`. `file_name` is RELATIVE to the clips directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipRecord {
    pub id: String,
    pub file_name: String,
    pub thumbnail_name: String,
    pub game_title: String,
    pub duration_ms: i64,
    pub file_size_bytes: i64,
    pub created_at: String,
    pub is_favorite: bool,
    pub drive_file_id: Option<String>,
    pub drive_web_url: Option<String>,
    /// Game folder the clip lives in ('' = legacy row still at the root).
    /// The association is stable: it survives app renames/removals.
    pub folder: String,
    /// Computed at query time: does the file still exist on disk?
    pub exists: bool,
}

/// One registered capture input (mirrors an OBS source we created).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredInput {
    pub id: String,
    pub input_name: String,
    /// 'window' | 'screen'
    pub input_kind: String,
    pub display_name: String,
    /// Window title learned from the picker (the autopilot matches by it).
    pub window_title: Option<String>,
    /// Window app id/class when the platform reports one (KDE), else NULL.
    pub window_app_id: Option<String>,
    /// Exe name stored from the Windows window target (unused on Linux).
    pub target_exe: String,
    /// Saved OBS source settings JSON (portal token / window target).
    pub input_settings: Option<String>,
    pub source_uuid: String,
    pub icon_path: Option<String>,
    /// Library folder for this game ('' until the first successful pick).
    pub clips_folder: String,
}
