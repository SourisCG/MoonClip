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
    /// Computed at query time: does the file still exist on disk?
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomApp {
    pub id: String,
    pub display_name: String,
    pub target_exe: String,
    /// 'exact_exe' | 'cmdline_contains' | 'window_title' | 'wine_target'
    pub match_strategy: String,
    pub clip_duration_seconds: Option<i64>,
    pub icon_path: Option<String>,
    pub is_wine_proton: bool,
    /// Wayland portal restore token for this registered game's window.
    pub portal_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterAppInput {
    pub display_name: String,
    pub target_exe: String,
    pub match_strategy: String,
    pub clip_duration_seconds: Option<i64>,
    pub is_wine_proton: Option<bool>,
}
