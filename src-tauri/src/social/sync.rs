//! Drive library recovery ("Restaurar biblioteca"): rebuilds the local
//! gallery from the app's remote tree so a second machine sees the same
//! clips Medal-style. Restored clips are cloud-only rows (the video stays in
//! Drive; opening one downloads it on demand) and their thumbnails come from
//! Drive's own generated thumbnails — no extra files are uploaded.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::storage::models::ClipRecord;
use crate::storage::DbState;

use super::drive::DriveFile;

const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const DEFAULT_FOLDER: &str = "Unknown";
/// Drive thumbnail size hint (`=s480`) appended to `thumbnailLink`.
const THUMB_SIZE: &str = "480";
/// One sync at a time (auto-on-connect + manual button).
static SYNCING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Default, Serialize)]
pub struct DriveSyncResult {
    pub folders: u32,
    pub scanned: u32,
    pub restored: u32,
    pub skipped: u32,
    pub thumbs_repaired: u32,
}

/// RFC3339 ("2026-09-27T18:04:05.123Z") -> SQLite "2026-09-27 18:04:05" so
/// restored rows sort together with local ones (`ORDER BY created_at DESC`).
pub fn sqlite_time(raw: &str) -> String {
    let trimmed = raw.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 19
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && (bytes[10] == b'T' || bytes[10] == b' ')
        && bytes[13] == b':'
        && bytes[16] == b':'
    {
        return format!("{} {}", &trimmed[..10], &trimmed[11..19]);
    }
    trimmed.to_string()
}

/// `...=s220` -> `...=s480` (Drive's thumbnail links accept a size suffix).
pub fn sized_thumbnail_link(link: &str, size: &str) -> String {
    if let Some(pos) = link.rfind("=s") {
        let suffix = &link[pos + 2..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return format!("{}=s{size}", &link[..pos]);
        }
    }
    format!("{link}=s{size}")
}

/// Drive folder names become local game folders: no path separators.
pub fn sanitize_folder(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' => '-',
            c => c,
        })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        DEFAULT_FOLDER.to_string()
    } else {
        cleaned
    }
}

fn split_name(name: &str) -> (String, String) {
    match name.rsplit_once('.') {
        Some((stem, ext)) => (stem.to_string(), format!(".{ext}")),
        None => (name.to_string(), String::new()),
    }
}

/// `name.mp4` or `name_2.mp4` when that relative path is already taken.
pub fn unique_name(taken: &HashSet<String>, folder: &str, name: &str) -> String {
    let key = |n: &str| format!("{folder}/{n}");
    if !taken.contains(&key(name)) {
        return name.to_string();
    }
    let (stem, ext) = split_name(name);
    for n in 2..1000 {
        let candidate = format!("{stem}_{n}{ext}");
        if !taken.contains(&key(&candidate)) {
            return candidate;
        }
    }
    name.to_string()
}

/// Fields of one restored cloud row (pure mapping, unit tested).
#[derive(Debug, Clone, PartialEq)]
pub struct RestoredRow {
    pub file_name: String,
    pub thumbnail_name: String,
    pub game_title: String,
    pub folder: String,
    pub duration_ms: i64,
    pub file_size_bytes: i64,
    pub created_at: String,
    pub drive_file_id: String,
    pub drive_web_url: Option<String>,
}

/// Map one remote video into a cloud row. `file_name` is the (already
/// de-duplicated) bare file name; the folder becomes the game.
pub fn restored_row(file: &DriveFile, folder: &str, file_name: &str) -> RestoredRow {
    let folder = sanitize_folder(folder);
    let bare = file_name.rsplit('/').next().unwrap_or(file_name);
    let (stem, _) = split_name(bare);
    let thumbnail_name = format!("{folder}/thumb_{stem}.jpg");
    let duration_ms = file
        .video_media_metadata
        .as_ref()
        .and_then(|meta| meta.duration_millis)
        .unwrap_or(0);
    let file_size_bytes = file
        .size
        .as_deref()
        .and_then(|size| size.parse::<i64>().ok())
        .unwrap_or(0);
    let created_at = file
        .created_time
        .as_deref()
        .or(file.modified_time.as_deref())
        .map(sqlite_time)
        .unwrap_or_default();
    RestoredRow {
        file_name: format!("{folder}/{bare}"),
        thumbnail_name,
        game_title: folder.clone(),
        folder,
        duration_ms,
        file_size_bytes,
        created_at,
        drive_file_id: file.id.clone(),
        drive_web_url: file.web_view_link.clone(),
    }
}

/// Download Drive's generated thumbnail (signed URL, no auth needed).
async fn download_thumbnail(
    http: &reqwest::Client,
    link: &str,
    dest: &Path,
) -> Result<(), String> {
    let response = http
        .get(sized_thumbnail_link(link, THUMB_SIZE))
        .send()
        .await
        .map_err(|e| format!("thumbnail request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("thumbnail download failed ({})", response.status()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("cannot read the thumbnail: {e}"))?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    tokio::fs::write(dest, &bytes)
        .await
        .map_err(|e| format!("cannot write {}: {e}", dest.display()))
}

/// Run the recovery. Emits `moonclip://drive-sync-done` with the summary.
pub async fn sync_library(app: &AppHandle) -> Result<DriveSyncResult, String> {
    if SYNCING.swap(true, Ordering::SeqCst) {
        return Err("a Drive sync is already running".into());
    }
    let result = sync_inner(app).await;
    SYNCING.store(false, Ordering::SeqCst);
    if let Ok(summary) = &result {
        let _ = app.emit("moonclip://drive-sync-done", summary);
    }
    result
}

async fn sync_inner(app: &AppHandle) -> Result<DriveSyncResult, String> {
    let client = super::drive_client_for(app).await?;
    let http = reqwest::Client::new();
    let mut result = DriveSyncResult::default();

    let (root, clips_dir, mut taken, known) = {
        let db = app.state::<DbState>();
        let root = super::commands::root_folder(&db, &client).await?;
        let clips = db.list_clips()?;
        let taken: HashSet<String> = clips.iter().map(|clip| clip.file_name.clone()).collect();
        let known: HashMap<String, ClipRecord> = clips
            .into_iter()
            .filter_map(|clip| {
                let id = clip.drive_file_id.clone().filter(|id| !id.is_empty())?;
                Some((id, clip))
            })
            .collect();
        (root, db.clips_dir()?, taken, known)
    };

    let mut queue: VecDeque<(String, String)> = VecDeque::new();
    queue.push_back((root, String::new()));
    while let Some((folder_id, folder_name)) = queue.pop_front() {
        let children = client.list_library_children(&folder_id).await?;
        for child in children {
            result.scanned += 1;
            if child.mime_type == FOLDER_MIME {
                let name = sanitize_folder(&child.name);
                {
                    let db = app.state::<DbState>();
                    db.upsert_drive_folder(&name, &child.id, &child.name)?;
                }
                result.folders += 1;
                queue.push_back((child.id, name));
                continue;
            }
            if !child.mime_type.starts_with("video/") {
                continue;
            }

            if let Some(record) = known.get(&child.id) {
                // Known clip: repair a missing local thumbnail (the row was
                // restored before Drive generated one, or the cache was lost).
                result.skipped += 1;
                let dest = clips_dir.join(&record.thumbnail_name);
                if !dest.is_file() {
                    if let Some(link) = &child.thumbnail_link {
                        if download_thumbnail(&http, link, &dest).await.is_ok() {
                            result.thumbs_repaired += 1;
                        }
                    }
                }
                continue;
            }

            let folder = if folder_name.is_empty() {
                DEFAULT_FOLDER.to_string()
            } else {
                folder_name.clone()
            };
            let name = unique_name(&taken, &folder, &child.name);
            let row = restored_row(&child, &folder, &name);
            if let Some(link) = &child.thumbnail_link {
                let dest = clips_dir.join(&row.thumbnail_name);
                let _ = download_thumbnail(&http, link, &dest).await;
            }
            let record = {
                let db = app.state::<DbState>();
                db.insert_cloud_clip(
                    &row.file_name,
                    &row.thumbnail_name,
                    &row.game_title,
                    row.duration_ms,
                    row.file_size_bytes,
                    &row.folder,
                    &row.drive_file_id,
                    row.drive_web_url.as_deref(),
                    &row.created_at,
                )?
            };
            taken.insert(record.file_name.clone());
            result.restored += 1;
        }
    }
    Ok(result)
}

/// Restore this machine's gallery from Drive: clips already uploaded are
/// indexed as cloud-only rows (video stays remote, downloaded on demand).
#[tauri::command]
pub async fn drive_sync_library(app: AppHandle) -> Result<DriveSyncResult, String> {
    sync_library(&app).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::social::drive::VideoMediaMetadata;

    fn drive_video() -> DriveFile {
        DriveFile {
            id: "file-1".into(),
            name: "Replay 2026-09-27 12-00-06_edit.mp4".into(),
            mime_type: "video/mp4".into(),
            size: Some("19654086".into()),
            modified_time: Some("2026-09-27T18:04:15.000Z".into()),
            created_time: Some("2026-09-27T18:04:05.123Z".into()),
            thumbnail_link: Some("https://lh3.googleusercontent.com/abc=s220".into()),
            web_view_link: Some("https://drive.google.com/file/d/file-1/view".into()),
            video_media_metadata: Some(VideoMediaMetadata {
                duration_millis: Some(8800),
                width: Some(1920),
                height: Some(1080),
            }),
        }
    }

    #[test]
    fn sqlite_time_normalizes_rfc3339() {
        assert_eq!(sqlite_time("2026-09-27T18:04:05.123Z"), "2026-09-27 18:04:05");
        assert_eq!(sqlite_time("2026-09-27T18:04:05Z"), "2026-09-27 18:04:05");
        assert_eq!(sqlite_time(" weird "), "weird");
    }

    #[test]
    fn thumbnail_links_get_a_bigger_size() {
        assert_eq!(
            sized_thumbnail_link("https://x/abc=s220", "480"),
            "https://x/abc=s480"
        );
        assert_eq!(
            sized_thumbnail_link("https://x/abc", "480"),
            "https://x/abc=s480"
        );
        assert_eq!(
            sized_thumbnail_link("https://x/abc=s220-c", "480"),
            "https://x/abc=s220-c=s480"
        );
    }

    #[test]
    fn folders_are_sanitized() {
        assert_eq!(sanitize_folder("  Overwatch  "), "Overwatch");
        assert_eq!(sanitize_folder("A/B"), "A-B");
        assert_eq!(sanitize_folder("   "), "Unknown");
        assert_eq!(sanitize_folder(".."), "Unknown");
    }

    #[test]
    fn unique_names_get_a_suffix_when_taken() {
        let mut taken = HashSet::new();
        assert_eq!(unique_name(&taken, "G", "clip.mp4"), "clip.mp4");
        taken.insert("G/clip.mp4".to_string());
        assert_eq!(unique_name(&taken, "G", "clip.mp4"), "clip_2.mp4");
        taken.insert("G/clip_2.mp4".to_string());
        assert_eq!(unique_name(&taken, "G", "clip.mp4"), "clip_3.mp4");
        assert_eq!(unique_name(&taken, "Other", "clip.mp4"), "clip.mp4");
    }

    #[test]
    fn drive_video_maps_into_a_cloud_row() {
        let row = restored_row(&drive_video(), "Undertale", "Replay.mp4");
        assert_eq!(row.file_name, "Undertale/Replay.mp4");
        assert_eq!(row.thumbnail_name, "Undertale/thumb_Replay.jpg");
        assert_eq!(row.game_title, "Undertale");
        assert_eq!(row.folder, "Undertale");
        assert_eq!(row.duration_ms, 8800);
        assert_eq!(row.file_size_bytes, 19654086);
        assert_eq!(row.created_at, "2026-09-27 18:04:05");
        assert_eq!(row.drive_file_id, "file-1");
        assert_eq!(
            row.drive_web_url.as_deref(),
            Some("https://drive.google.com/file/d/file-1/view")
        );
    }

    #[test]
    fn missing_metadata_falls_back_gracefully() {
        let mut file = drive_video();
        file.size = None;
        file.created_time = None;
        file.video_media_metadata = None;
        file.web_view_link = None;
        let row = restored_row(&file, "", "clip.mp4");
        assert_eq!(row.folder, "Unknown");
        assert_eq!(row.file_name, "Unknown/clip.mp4");
        assert_eq!(row.duration_ms, 0);
        assert_eq!(row.file_size_bytes, 0);
        // Falls back to modifiedTime.
        assert_eq!(row.created_at, "2026-09-27 18:04:15");
        assert!(row.drive_web_url.is_none());
    }
}
