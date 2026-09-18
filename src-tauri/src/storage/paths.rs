//! Filesystem path helpers. The DB never stores absolute paths.
//! The platform default clips home lives in `crate::os::paths`
//! (Windows: AV-safe %LOCALAPPDATA% home; Linux: ~/Videos) so shared code
//! never branches on OS.

use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// Resolve the physical path of a clip from the configured base dir.
pub fn resolve_clip_path(base_dir: &Path, file_name: &str) -> PathBuf {
    base_dir.join(file_name)
}

/// Platform default clips directory (see `crate::os::paths`).
pub fn default_clips_dir() -> PathBuf {
    crate::os::paths::default_clips_dir()
}

/// One-time relocation of a legacy library: moves our own files
/// (`*.mp4` + `thumb_*.jpg`, top level only) from `old` to `new`, creating
/// `new` first. Unknown files and subdirectories are left untouched.
/// Returns files moved. DB rows need no migration (relative names).
pub fn migrate_legacy_clips_dir(old: &Path, new: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(new).map_err(|e| format!("cannot create clips dir: {e}"))?;
    let entries = std::fs::read_dir(old).map_err(|e| format!("cannot read legacy dir: {e}"))?;
    let mut moved = 0usize;
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let ours = name.ends_with(".mp4") || (name.starts_with("thumb_") && name.ends_with(".jpg"));
        if !ours {
            continue;
        }
        let dest = new.join(name);
        if dest.exists() {
            continue;
        }
        if std::fs::rename(&path, &dest).is_err() {
            // Cross-volume fallback: copy + delete.
            std::fs::copy(&path, &dest).map_err(|e| format!("cannot move {name}: {e}"))?;
            let _ = std::fs::remove_file(&path);
        }
        moved += 1;
    }
    Ok(moved)
}

/// SQLite file location: <app_data>/moonclip.db
pub fn db_file_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir unavailable: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create app data dir: {e}"))?;
    Ok(dir.join("moonclip.db"))
}

#[cfg(test)]
mod tests {
    use super::migrate_legacy_clips_dir;

    #[test]
    fn migrates_only_ours() {
        let base = std::env::temp_dir().join(format!("moonclip-mig-{}", std::process::id()));
        let old = base.join("old");
        let new = base.join("new");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("replay_a.mp4"), b"v").unwrap();
        std::fs::write(old.join("thumb_replay_a.jpg"), b"t").unwrap();
        std::fs::write(old.join("notes.txt"), b"keep").unwrap();
        let n = migrate_legacy_clips_dir(&old, &new).unwrap();
        assert_eq!(n, 2);
        assert!(new.join("replay_a.mp4").exists());
        assert!(new.join("thumb_replay_a.jpg").exists());
        assert!(old.join("notes.txt").exists());
        assert!(!old.join("replay_a.mp4").exists());
        std::fs::remove_dir_all(&base).ok();
    }
}
