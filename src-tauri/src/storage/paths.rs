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

/// A relative media name: `<file>` or `<folder>/<file>` (exactly one level,
/// which is what the game-folder layout uses). Rejects traversal, absolute
/// paths, backslashes, drive prefixes, empty/dotted segments, NUL and
/// leading/trailing whitespace.
pub fn is_safe_relative_media_name(name: &str) -> bool {
    if name.is_empty() || name.contains('\\') || name.contains('\0') {
        return false;
    }
    if Path::new(name).is_absolute() {
        return false;
    }
    let segments: Vec<&str> = name.split('/').collect();
    if segments.len() > 2 {
        return false;
    }
    segments.iter().all(|seg| {
        !seg.is_empty()
            && *seg != "."
            && *seg != ".."
            && !seg.contains(':')
            && seg.trim() == *seg
    })
}

/// Platform default clips directory (see `crate::os::paths`).
pub fn default_clips_dir() -> PathBuf {
    crate::os::paths::default_clips_dir()
}

/// One-time relocation of a legacy library: moves our own files
/// (`*.mp4`/`*.mkv` + `thumb_*.jpg`) and the game subfolders from `old` to
/// `new`, creating `new` first. Unknown files at the root are left untouched.
/// Returns files moved. DB rows need no migration (relative names).
pub fn migrate_legacy_clips_dir(old: &Path, new: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(new).map_err(|e| format!("cannot create clips dir: {e}"))?;
    let entries = std::fs::read_dir(old).map_err(|e| format!("cannot read legacy dir: {e}"))?;
    let mut moved = 0usize;
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            // Game folders travel whole (recursive).
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let dest = new.join(name);
            if dest.exists() {
                continue;
            }
            moved += move_tree(&path, &dest)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let lower = name.to_ascii_lowercase();
        let ours = lower.ends_with(".mp4")
            || lower.ends_with(".mkv")
            || (name.starts_with("thumb_") && lower.ends_with(".jpg"));
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

/// Recursively move a directory (rename, then copy+delete across volumes).
fn move_tree(src: &Path, dst: &Path) -> Result<usize, String> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(count_files(dst));
    }
    std::fs::create_dir_all(dst).map_err(|e| format!("cannot create {}: {e}", dst.display()))?;
    let mut moved = 0usize;
    let entries =
        std::fs::read_dir(src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            moved += move_tree(&path, &target)?;
        } else if std::fs::copy(&path, &target).is_ok() {
            let _ = std::fs::remove_file(&path);
            moved += 1;
        }
    }
    let _ = std::fs::remove_dir_all(src);
    Ok(moved)
}

fn count_files(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| {
            let path = e.path();
            if path.is_dir() {
                count_files(&path)
            } else {
                1
            }
        })
        .sum()
}

/// SQLite file location: <app_data>/moonclip.db
pub fn db_file_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir unavailable: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create app data dir: {e}"))?;
    let _ = harden_dir(&dir);
    Ok(dir.join("moonclip.db"))
}

/// Owner-only permissions for MoonClip-owned directories (DB, edits, engine
/// config). Never applied to the user's clips directory: that location is
/// theirs and may be shared. No-op on Windows (inherited ACLs).
#[cfg(unix)]
pub fn harden_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub fn harden_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Owner-only permissions for MoonClip-owned files (the DB, session data).
#[cfg(unix)]
pub fn harden_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
pub fn harden_file(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::migrate_legacy_clips_dir;
    use super::is_safe_relative_media_name;

    #[test]
    fn safe_names_accept_files_and_one_folder() {
        assert!(is_safe_relative_media_name("Replay 2026-01-01 00-00-00.mp4"));
        assert!(is_safe_relative_media_name("Kingdom Hearts/Replay 2026.mp4"));
        assert!(is_safe_relative_media_name("thumb_Replay 2026.jpg"));
    }

    #[test]
    fn unsafe_names_are_rejected() {
        for bad in [
            "",
            "..",
            "../evil.mp4",
            "a/../evil.mp4",
            "a/../../evil.mp4",
            "a//b.mp4",
            "a/./b.mp4",
            "/abs.mp4",
            "\\unc\\evil.mp4",
            "a\\b.mp4",
            "C:/evil.mp4",
            "a/b/c.mp4",
            "trailing/",
            "/leading",
            " lead.mp4",
            "trail.mp4 ",
            "nul\0.mp4",
        ] {
            assert!(!is_safe_relative_media_name(bad), "should reject {bad:?}");
        }
    }

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

    #[test]
    fn migration_carries_game_folders_whole() {
        let base =
            std::env::temp_dir().join(format!("moonclip-mig-dir-{}", uuid::Uuid::new_v4().simple()));
        let old = base.join("old");
        let new = base.join("new");
        std::fs::create_dir_all(old.join("Kingdom Hearts")).unwrap();
        std::fs::write(old.join("Kingdom Hearts/Replay 2026.mp4"), b"v").unwrap();
        std::fs::write(old.join("Kingdom Hearts/thumb_Replay 2026.jpg"), b"t").unwrap();
        std::fs::write(old.join("root.mp4"), b"r").unwrap();
        let n = migrate_legacy_clips_dir(&old, &new).unwrap();
        assert_eq!(n, 3);
        assert!(new.join("Kingdom Hearts/Replay 2026.mp4").exists());
        assert!(new.join("Kingdom Hearts/thumb_Replay 2026.jpg").exists());
        assert!(new.join("root.mp4").exists());
        assert!(!old.join("Kingdom Hearts").exists());
        std::fs::remove_dir_all(&base).ok();
    }
}
