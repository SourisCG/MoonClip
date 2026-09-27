//! Library reconciliation (docs/05_STORAGE_SECURITY.md §3): index files that
//! exist on disk but have no DB row. Files are NEVER deleted here; orphan DB
//! rows keep the explicit `purge_missing_clips` command (external drives can
//! be temporarily unavailable).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::DbState;

/// Result of one reconciliation pass.
#[derive(Debug, Default, serde::Serialize)]
pub struct ReconcileReport {
    pub scanned: u32,
    pub indexed: u32,
    pub failed: u32,
}

/// MoonClip-style names at the top level. Derived clips (`*_trim`, `*_edit`)
/// and OBS replay files (`Replay <timestamp>`) only; anything else the user
/// drops at the root of the library is left alone.
fn is_moonclip_clip_name(name: &str) -> bool {
    if !has_video_ext(name) {
        return false;
    }
    let stem = stem_of(name);
    stem.starts_with("Replay ") || stem.contains("_trim") || stem.contains("_edit")
}

fn has_video_ext(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".mp4") || lower.ends_with(".mkv")
}

fn stem_of(name: &str) -> &str {
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

/// Relative names (POSIX separators) of indexable videos: MoonClip-style at
/// the root, any video one level deep (inside a game folder).
pub fn scan_candidates(base: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(base) else {
        return found;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if path.is_file() {
            if is_moonclip_clip_name(name) {
                found.push(name.to_string());
            }
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        // Game folders: one level, any video inside is ours.
        let Ok(children) = std::fs::read_dir(&path) else {
            continue;
        };
        for child in children.filter_map(|e| e.ok()) {
            let child_path = child.path();
            if !child_path.is_file() {
                continue;
            }
            let Some(child_name) = child_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if has_video_ext(child_name) {
                found.push(format!("{name}/{child_name}"));
            }
        }
    }
    found.sort();
    found
}

/// Thumbnail path for a clip, next to the video: `thumb_<stem>.jpg`.
pub fn thumbnail_name_for(file_name: &str) -> String {
    match file_name.rsplit_once('/') {
        Some((folder, name)) => format!("{folder}/thumb_{}.jpg", stem_of(name)),
        None => format!("thumb_{}.jpg", stem_of(file_name)),
    }
}

/// Index every candidate missing from the DB. Returns what happened.
/// `db` is behind a Mutex; this is meant to run in the background at boot and
/// from the manual command.
pub async fn reconcile_dir(db: &DbState, ffmpeg: &Path) -> Result<ReconcileReport, String> {
    let base = db.clips_dir()?;
    let existing: HashSet<String> = db
        .list_clips()?
        .into_iter()
        .map(|c| c.file_name)
        .collect();
    let mut report = ReconcileReport::default();
    for rel in scan_candidates(&base) {
        if existing.contains(&rel) {
            continue;
        }
        report.scanned += 1;
        let path: PathBuf = base.join(&rel);
        let thumb_rel = thumbnail_name_for(&rel);
        let thumb_path = base.join(&thumb_rel);
        let duration = crate::editor::ffmpeg::probe_duration_ms(ffmpeg, &path)
            .await
            .unwrap_or(0);
        if !thumb_path.exists() {
            let t = if duration > 0 && duration < 1500 {
                0.05
            } else {
                1.0
            };
            if let Err(e) =
                crate::editor::ffmpeg::make_thumbnail(ffmpeg, &path, &thumb_path, t).await
            {
                eprintln!("[moonclip] reconcile: cannot thumbnail {rel}: {e}");
                report.failed += 1;
                continue;
            }
        }
        let folder = match rel.rsplit_once('/') {
            Some((folder, _)) => folder.to_string(),
            None => String::new(),
        };
        let game_title = if folder.is_empty() {
            "Unknown".to_string()
        } else {
            folder.clone()
        };
        let size = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
        match db.insert_clip(&rel, &thumb_rel, &game_title, duration, size, &folder) {
            Ok(_) => {
                report.indexed += 1;
                eprintln!("[moonclip] reconcile: indexed {rel}");
            }
            Err(e) => {
                eprintln!("[moonclip] reconcile: cannot index {rel}: {e}");
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

/// Result of one library organization pass.
#[derive(Debug, Default, serde::Serialize)]
pub struct OrganizeReport {
    pub moved: u32,
    pub skipped: u32,
    pub failed: u32,
}

/// Move legacy flat clips (folder = '') into their game folder, updating the
/// DB names to match the disk. Idempotent; a clip whose file is missing is
/// left untouched (the explicit purge owns orphan rows). Never deletes.
pub fn organize_dir(db: &DbState) -> Result<OrganizeReport, String> {
    let base = db.clips_dir()?;
    let mut report = OrganizeReport::default();
    for clip in db.list_clips()? {
        if !clip.folder.trim().is_empty() {
            continue;
        }
        let source = base.join(&clip.file_name);
        if !source.is_file() {
            report.skipped += 1;
            continue;
        }
        let folder = crate::storage::folders::sanitize_game_folder(&clip.game_title);
        let dir = base.join(&folder);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("[moonclip] organize: cannot create {}: {e}", dir.display());
            report.failed += 1;
            continue;
        }
        let Some(bare) = source.file_name().and_then(|n| n.to_str()) else {
            report.failed += 1;
            continue;
        };
        let dest = dir.join(bare);
        if dest != source {
            if let Err(e) = std::fs::rename(&source, &dest) {
                if std::fs::copy(&source, &dest).is_err() {
                    eprintln!("[moonclip] organize: cannot move {}: {e}", source.display());
                    report.failed += 1;
                    continue;
                }
                let _ = std::fs::remove_file(&source);
            }
        }
        let thumb_source = base.join(&clip.thumbnail_name);
        let thumb_bare = thumb_source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let thumb_name = format!("{folder}/{thumb_bare}");
        if thumb_source.is_file() {
            let thumb_dest = dir.join(&thumb_bare);
            if thumb_dest != thumb_source && std::fs::rename(&thumb_source, &thumb_dest).is_err() {
                let _ = std::fs::copy(&thumb_source, &thumb_dest);
                let _ = std::fs::remove_file(&thumb_source);
            }
        }
        let file_name = format!("{folder}/{bare}");
        if let Err(e) = db.set_clip_folder(&clip.id, &folder, &file_name, &thumb_name) {
            eprintln!("[moonclip] organize: cannot update row {}: {e}", clip.file_name);
            report.failed += 1;
            continue;
        }
        report.moved += 1;
        eprintln!("[moonclip] organize: {} -> {}", clip.file_name, file_name);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moonclip_names_are_recognized() {
        assert!(is_moonclip_clip_name("Replay 2026-09-26 18-13-22.mp4"));
        assert!(is_moonclip_clip_name(
            "Replay 2026-09-26 18-13-22_2.mp4"
        ));
        assert!(is_moonclip_clip_name("Replay 2026-09-26 18-13-22_trim.mkv"));
        assert!(is_moonclip_clip_name("Replay 2026-09-26 18-13-22_edit_3.mp4"));
        assert!(!is_moonclip_clip_name("holiday.mp4"));
        assert!(!is_moonclip_clip_name("thumb_Replay 2026.mp4"));
        assert!(!is_moonclip_clip_name("Replay 2026.txt"));
    }

    #[test]
    fn scan_takes_root_patterns_and_any_video_in_game_folders() {
        let dir = std::env::temp_dir().join(format!(
            "moonclip-scan-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let game = dir.join("Kingdom Hearts");
        std::fs::create_dir_all(&game).unwrap();
        for (rel, _) in [
            ("Replay 2026-01-01 00-00-00.mp4", true),
            ("holiday.mp4", false),
            ("thumb_Replay 2026-01-01 00-00-00.jpg", false),
            ("notes.txt", false),
            ("Kingdom Hearts/Replay 2026-01-02 00-00-00.MP4", true),
            ("Kingdom Hearts/random clip.mkv", true),
            ("Kingdom Hearts/thumb_x.jpg", false),
        ] {
            let p = dir.join(rel);
            std::fs::write(&p, b"x").unwrap();
        }
        let found = scan_candidates(&dir);
        assert_eq!(
            found,
            vec![
                "Kingdom Hearts/Replay 2026-01-02 00-00-00.MP4".to_string(),
                "Kingdom Hearts/random clip.mkv".to_string(),
                "Replay 2026-01-01 00-00-00.mp4".to_string(),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live acceptance: a real file dropped in a game folder is indexed once
    /// (duration + thumbnail), and a second pass is a no-op.
    #[tokio::test]
    async fn live_reconcile_indexes_files_once() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let ff = manifest
            .join("binaries")
            .join(crate::sidecar::host_triple())
            .join(crate::editor::ffmpeg::sidecar_name());
        if !ff.exists() {
            eprintln!("skip: bundled ffmpeg not staged");
            return;
        }
        let db = crate::storage::db::test_db();
        let dir = std::env::temp_dir().join(format!(
            "moonclip-reconcile-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let game = dir.join("Game A");
        std::fs::create_dir_all(&game).unwrap();
        db.set_setting("clips_directory", dir.to_str().unwrap())
            .unwrap();
        let video = game.join("Replay 2026-01-01 00-00-00.mp4");
        let ok = tokio::process::Command::new(&ff)
            .args([
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=30",
                "-t",
                "1",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
            ])
            .arg(&video)
            .status()
            .await
            .unwrap();
        assert!(ok.success(), "fixture encode failed");

        let report = reconcile_dir(&db, &ff).await.unwrap();
        assert_eq!(report.indexed, 1, "{report:?}");
        let clips = db.list_clips().unwrap();
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].file_name, "Game A/Replay 2026-01-01 00-00-00.mp4");
        assert_eq!(clips[0].game_title, "Game A");
        assert!(clips[0].duration_ms > 0, "duration not probed");
        assert!(game.join("thumb_Replay 2026-01-01 00-00-00.jpg").exists());

        let again = reconcile_dir(&db, &ff).await.unwrap();
        assert_eq!(again.indexed, 0);
        assert_eq!(again.scanned, 0);
        assert_eq!(db.list_clips().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn organize_moves_flat_clips_into_game_folders() {
        let db = crate::storage::db::test_db();
        let dir = std::env::temp_dir().join(format!(
            "moonclip-organize-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        db.set_setting("clips_directory", dir.to_str().unwrap())
            .unwrap();
        std::fs::write(dir.join("Replay 2026.mp4"), b"v").unwrap();
        std::fs::write(dir.join("thumb_Replay 2026.jpg"), b"t").unwrap();
        let clip = db
            .insert_clip(
                "Replay 2026.mp4",
                "thumb_Replay 2026.jpg",
                "DOOM: Eternal",
                1000,
                10,
                "",
            )
            .unwrap();
        let report = organize_dir(&db).unwrap();
        assert_eq!(report.moved, 1, "{report:?}");
        assert!(!dir.join("Replay 2026.mp4").exists());
        assert!(dir.join("DOOM- Eternal/Replay 2026.mp4").exists());
        assert!(dir.join("DOOM- Eternal/thumb_Replay 2026.jpg").exists());
        let updated = db
            .list_clips()
            .unwrap()
            .into_iter()
            .find(|c| c.id == clip.id)
            .unwrap();
        assert_eq!(updated.folder, "DOOM- Eternal");
        assert_eq!(updated.file_name, "DOOM- Eternal/Replay 2026.mp4");
        assert_eq!(updated.thumbnail_name, "DOOM- Eternal/thumb_Replay 2026.jpg");
        // Second pass is a no-op.
        assert_eq!(organize_dir(&db).unwrap().moved, 0);
        // Missing files never break the pass (or delete the row).
        db.insert_clip("gone.mp4", "t.jpg", "G", 1, 1, "").unwrap();
        assert_eq!(organize_dir(&db).unwrap().skipped, 1);
        assert_eq!(db.list_clips().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn thumbnails_live_next_to_their_video() {
        assert_eq!(
            thumbnail_name_for("Replay 2026-01-01 00-00-00.mp4"),
            "thumb_Replay 2026-01-01 00-00-00.jpg"
        );
        assert_eq!(
            thumbnail_name_for("Kingdom Hearts/Replay 2026-01-01 00-00-00.mkv"),
            "Kingdom Hearts/thumb_Replay 2026-01-01 00-00-00.jpg"
        );
    }
}
