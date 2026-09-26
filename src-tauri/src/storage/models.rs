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
    /// Stable identity for detected games (`steam:570`, `prism:foo`, ...).
    pub game_key: Option<String>,
    /// 'window' | 'monitor'
    pub capture_mode: String,
    /// 'x11' | 'portal' | 'monitor' | NULL
    pub source_kind: Option<String>,
    /// Ready-to-use `xcomposite_input` match string for X11 windows.
    pub window_match: Option<String>,
    /// Wayland-native window restore token for this game.
    pub portal_token: Option<String>,
    /// Medal-style auto buffer for this app.
    pub auto_buffer: bool,
    pub last_seen_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterAppInput {
    pub display_name: String,
    pub target_exe: String,
    pub match_strategy: String,
    pub clip_duration_seconds: Option<i64>,
    pub is_wine_proton: Option<bool>,
    pub game_key: Option<String>,
    pub source_kind: Option<String>,
    pub window_match: Option<String>,
    pub auto_buffer: Option<bool>,
}
