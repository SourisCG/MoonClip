//! Rename a clip: the video file, its thumbnail and the DB row. The game
//! folder and the file extension never change; a name that already exists in
//! the same folder is an error (no silent suffixing).

use std::path::{Path, PathBuf};

use super::folders::sanitize_file_stem;
use super::models::ClipRecord;
use super::paths;
use super::DbState;

/// Rename the clip's file stem. Same stem = no-op; the DB is only touched
/// after the disk move succeeds.
pub fn rename_clip(db: &DbState, clip_id: &str, new_stem: &str) -> Result<ClipRecord, String> {
    let base = db.clips_dir()?;
    let clips = db.list_clips()?;
    let clip = clips
        .iter()
        .find(|c| c.id == clip_id)
        .ok_or_else(|| "clip not found".to_string())?;
    let stem = sanitize_file_stem(new_stem).ok_or_else(|| "invalid clip name".to_string())?;
    let old_path = paths::resolve_clip_path(&base, &clip.file_name);
    if !old_path.is_file() {
        return Err(format!("clip file missing: {}", clip.file_name));
    }
    let old_stem = old_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("bad clip file name")?;
    if stem == old_stem {
        return Ok(clip.clone());
    }
    let ext = old_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("mp4")
        .to_string();
    let folder = clip.folder.trim().to_string();
    let dir = if folder.is_empty() {
        base.clone()
    } else {
        base.join(&folder)
    };
    let bare = format!("{stem}.{ext}");
    let thumb_bare = format!("thumb_{stem}.jpg");
    let (file_name, thumbnail_name) = if folder.is_empty() {
        (bare.clone(), thumb_bare.clone())
    } else {
        (format!("{folder}/{bare}"), format!("{folder}/{thumb_bare}"))
    };
    let new_path = dir.join(&bare);
    // Collision with another clip's name or with a file already on disk.
    if clips
        .iter()
        .any(|c| c.id != clip_id && c.file_name == file_name)
        || new_path.exists()
    {
        return Err("a clip with that name already exists".into());
    }
    move_file(&old_path, &new_path)?;
    let old_thumb = paths::resolve_clip_path(&base, &clip.thumbnail_name);
    if old_thumb.is_file() {
        move_file(&old_thumb, &dir.join(&thumb_bare))?;
    }
    db.set_clip_folder(clip_id, &folder, &file_name, &thumbnail_name)?;
    db.list_clips()?
        .into_iter()
        .find(|c| c.id == clip_id)
        .ok_or_else(|| "clip vanished after rename".to_string())
}

fn move_file(from: &Path, to: &PathBuf) -> Result<(), String> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|e| format!("cannot rename {}: {e}", from.display()))?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db::test_db;

    fn setup() -> (DbState, PathBuf) {
        let db = test_db();
        let dir = std::env::temp_dir().join(format!(
            "moonclip-rename-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(dir.join("Overwatch")).unwrap();
        db.set_setting("clips_directory", dir.to_str().unwrap())
            .unwrap();
        std::fs::write(dir.join("Overwatch/Replay 2026.mp4"), b"v").unwrap();
        std::fs::write(dir.join("Overwatch/thumb_Replay 2026.jpg"), b"t").unwrap();
        (db, dir)
    }

    #[test]
    fn rename_moves_the_file_and_thumbnail_and_updates_the_row() {
        let (db, dir) = setup();
        let clip = db
            .insert_clip(
                "Overwatch/Replay 2026.mp4",
                "Overwatch/thumb_Replay 2026.jpg",
                "Overwatch",
                1000,
                10,
                "Overwatch",
            )
            .unwrap();
        let renamed = rename_clip(&db, &clip.id, "Final kill").unwrap();
        assert_eq!(renamed.file_name, "Overwatch/Final kill.mp4");
        assert_eq!(renamed.thumbnail_name, "Overwatch/thumb_Final kill.jpg");
        assert_eq!(renamed.folder, "Overwatch");
        assert_eq!(renamed.game_title, "Overwatch");
        assert!(dir.join("Overwatch/Final kill.mp4").exists());
        assert!(dir.join("Overwatch/thumb_Final kill.jpg").exists());
        assert!(!dir.join("Overwatch/Replay 2026.mp4").exists());
        // Persisted, not just returned.
        let stored = db.list_clips().unwrap();
        assert_eq!(stored[0].file_name, "Overwatch/Final kill.mp4");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_rejects_collisions_and_invalid_names() {
        let (db, dir) = setup();
        std::fs::write(dir.join("Overwatch/Other.mp4"), b"v").unwrap();
        let a = db
            .insert_clip(
                "Overwatch/Replay 2026.mp4",
                "Overwatch/thumb_Replay 2026.jpg",
                "Overwatch",
                1000,
                10,
                "Overwatch",
            )
            .unwrap();
        db.insert_clip(
            "Overwatch/Other.mp4",
            "Overwatch/thumb_Other.jpg",
            "Overwatch",
            1000,
            10,
            "Overwatch",
        )
        .unwrap();
        assert!(rename_clip(&db, &a.id, "Other").is_err());
        assert!(rename_clip(&db, &a.id, "   ").is_err());
        // Traversal-looking input is neutralized by the sanitizer, never used
        // as a path.
        assert_eq!(sanitize_file_stem("../evil").as_deref(), Some("-evil"));
        // Nothing moved, nothing changed.
        assert!(dir.join("Overwatch/Replay 2026.mp4").exists());
        assert_eq!(
            db.list_clips()
                .unwrap()
                .iter()
                .find(|c| c.id == a.id)
                .unwrap()
                .file_name,
            "Overwatch/Replay 2026.mp4"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_to_the_same_name_is_a_noop_and_missing_files_error() {
        let (db, dir) = setup();
        let clip = db
            .insert_clip(
                "Overwatch/Replay 2026.mp4",
                "Overwatch/thumb_Replay 2026.jpg",
                "Overwatch",
                1000,
                10,
                "Overwatch",
            )
            .unwrap();
        let same = rename_clip(&db, &clip.id, "Replay 2026").unwrap();
        assert_eq!(same.file_name, "Overwatch/Replay 2026.mp4");
        std::fs::remove_file(dir.join("Overwatch/Replay 2026.mp4")).unwrap();
        assert!(rename_clip(&db, &clip.id, "Ghost").is_err());
        assert_eq!(
            db.list_clips().unwrap()[0].file_name,
            "Overwatch/Replay 2026.mp4"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
