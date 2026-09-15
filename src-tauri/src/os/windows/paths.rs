//! Default library locations (Windows): the app's own folder under
//! %LOCALAPPDATA%, deliberately OUTSIDE the user media folders.
//!
//! Rationale: ~/Videos (and Documents/Desktop) sit under Controlled Folder
//! Access (ransomware protection) and are often redirected into OneDrive.
//! An unsigned clip app mass-writing `.mp4` there is blocked or flagged by
//! Defender/third-party AV and syncs every clip to the cloud.
//! `%LOCALAPPDATA%\MoonClip\Clips` is per-user, needs no elevation, is never
//! synced, and is not CFA-protected. Users can still pick any folder in
//! Settings (the folder picker path is untouched).

use std::path::PathBuf;

/// Default clips directory: %LOCALAPPDATA%/MoonClip/Clips.
pub fn default_clips_dir() -> PathBuf {
    if let Some(local) = dirs::data_local_dir() {
        return local.join("MoonClip").join("Clips");
    }
    if let Some(videos) = dirs::video_dir() {
        return videos.join("MoonClip");
    }
    PathBuf::from("MoonClip")
}

/// Pre-relocation default (~/Videos/MoonClip). When the stored setting still
/// equals this, a one-time migration moves the files to the new home.
/// `None` would mean "no legacy location" (Linux uses that).
pub fn legacy_default_clips_dir() -> Option<PathBuf> {
    dirs::video_dir().map(|v| v.join("MoonClip"))
}
