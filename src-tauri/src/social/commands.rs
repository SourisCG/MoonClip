//! Drive commands: upload a clip (private by default, optional public link),
//! browse the app's remote folders and download clips back into the local
//! game folders (indexed like any other clip).

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::storage::models::ClipRecord;
use crate::storage::DbState;

use super::drive::{DriveClient, DriveFile};

#[derive(Debug, Clone, Serialize)]
pub struct DriveUploadResult {
    pub file_id: String,
    pub name: String,
    pub web_link: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriveEntry {
    pub id: String,
    pub name: String,
    pub is_folder: bool,
    pub size: Option<i64>,
    pub modified_time: Option<String>,
}

impl From<DriveFile> for DriveEntry {
    fn from(file: DriveFile) -> Self {
        let is_folder = file.mime_type == "application/vnd.google-apps.folder";
        Self {
            id: file.id,
            name: file.name,
            is_folder,
            size: file.size.and_then(|s| s.parse().ok()),
            modified_time: file.modified_time,
        }
    }
}

/// Root folder id (cached in settings) or a fresh one.
async fn root_folder(db: &DbState, client: &DriveClient) -> Result<String, String> {
    let cached = db
        .get_settings()?
        .get("drive_root_folder_id")
        .cloned()
        .unwrap_or_default();
    if !cached.trim().is_empty() {
        return Ok(cached);
    }
    super::drive::ensure_root_folder(db, client).await
}

/// Remote folder mirroring a local game folder (cached mapping).
async fn remote_folder(
    db: &DbState,
    client: &DriveClient,
    root: &str,
    folder: &str,
) -> Result<String, String> {
    if let Some(id) = db.drive_folder(folder)? {
        return Ok(id);
    }
    let found = client.ensure_folder(folder, Some(root)).await?;
    db.upsert_drive_folder(folder, &found.id, &found.name)?;
    Ok(found.id)
}

/// Avoid clobbering a remote file with the same name.
async fn unique_remote_name(
    client: &DriveClient,
    parent: &str,
    name: &str,
) -> Result<String, String> {
    if client.find_file(name, parent).await?.is_none() {
        return Ok(name.to_string());
    }
    let (stem, ext) = split_name(name);
    for n in 2..1000 {
        let candidate = format!("{stem}_{n}{ext}");
        if client.find_file(&candidate, parent).await?.is_none() {
            return Ok(candidate);
        }
    }
    Err("cannot find a free name in Drive".into())
}

fn split_name(name: &str) -> (String, String) {
    match name.rsplit_once('.') {
        Some((stem, ext)) => (stem.to_string(), format!(".{ext}")),
        None => (name.to_string(), String::new()),
    }
}

/// Upload one clip into `MoonClip/<game>/`, optionally making it public.
#[tauri::command]
pub async fn drive_upload_clip(
    app: AppHandle,
    clip_id: String,
    make_public: bool,
    delete_local: bool,
    replace: bool,
) -> Result<DriveUploadResult, String> {
    let client = super::drive_client_for(&app).await?;
    let db = app.state::<DbState>();
    let clip = db
        .list_clips()?
        .into_iter()
        .find(|c| c.id == clip_id)
        .ok_or_else(|| "clip not found".to_string())?;
    let bare_name = clip
        .file_name
        .rsplit('/')
        .next()
        .ok_or("bad clip file name")?
        .to_string();

    let existing = clip.drive_file_id.clone().filter(|id| !id.is_empty());
    if let Some(file_id) = existing {
        if replace {
            // Explicit replace: trash the old copy so the name stays free.
            client.trash_file(&file_id).await?;
            db.clear_clip_drive(&clip_id)?;
        } else if let Some(file) = client.live_file(&file_id).await? {
            // Already in Drive: NEVER upload a duplicate. Only the link state
            // (or deleting the local copy) may change.
            let mut web_link = clip.drive_web_url.clone();
            if make_public && web_link.is_none() {
                client.set_public(&file_id).await?;
                web_link = web_view_link(&client, &file_id).await;
                db.set_clip_drive(&clip_id, &file_id, web_link.as_deref())?;
            }
            if delete_local && !clip.cloud {
                delete_local_video(&db, &clip).await;
                db.set_clip_cloud(&clip_id, &file_id, web_link.as_deref())?;
            }
            return Ok(DriveUploadResult {
                file_id,
                name: file.name,
                web_link,
            });
        } else {
            // Gone from Drive: forget the ids and upload fresh.
            db.clear_clip_drive(&clip_id)?;
        }
    }

    let base = db.clips_dir()?;
    let path = crate::commands::validated_media_path(&base, &clip.file_name)?;
    let folder = if clip.folder.trim().is_empty() {
        "Unknown".to_string()
    } else {
        clip.folder.clone()
    };
    let root = root_folder(&db, &client).await?;
    let parent = remote_folder(&db, &client, &root, &folder).await?;
    let name = unique_remote_name(&client, &parent, &bare_name).await?;
    let emit_app = app.clone();
    let emit_id = clip_id.clone();
    let file = client
        .upload_file(&path, &name, &parent, "video/mp4", move |sent, total| {
            let _ = emit_app.emit(
                "moonclip://upload-progress",
                serde_json::json!({"clipId": emit_id, "sent": sent, "total": total}),
            );
        })
        .await?;
    let mut web_link = None;
    if make_public {
        client.set_public(&file.id).await?;
        web_link = web_view_link(&client, &file.id).await;
    }
    // Always remember the upload so it can never be duplicated.
    db.set_clip_drive(&clip_id, &file.id, web_link.as_deref())?;
    if delete_local {
        delete_local_video(&db, &clip).await;
        db.set_clip_cloud(&clip_id, &file.id, web_link.as_deref())?;
    }
    Ok(DriveUploadResult {
        file_id: file.id,
        name: file.name,
        web_link,
    })
}

async fn web_view_link(client: &DriveClient, file_id: &str) -> Option<String> {
    client
        .raw_file(file_id, "webViewLink")
        .await
        .ok()
        .and_then(|value| {
            value
                .get("webViewLink")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
}

/// Medal-style: keep only the thumbnail locally so the gallery still renders.
async fn delete_local_video(db: &DbState, clip: &ClipRecord) {
    let Ok(base) = db.clips_dir() else { return };
    let path = crate::storage::paths::resolve_clip_path(&base, &clip.file_name);
    if let Err(e) = tokio::fs::remove_file(&path).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("[moonclip] could not delete the local video: {e}");
        }
    }
}

/// Browse the app's Drive tree (root when `folder_id` is None).
#[tauri::command]
pub async fn drive_browse(app: AppHandle, folder_id: Option<String>) -> Result<Vec<DriveEntry>, String> {
    let client = super::drive_client_for(&app).await?;
    let parent = match folder_id {
        Some(id) if !id.trim().is_empty() => id,
        _ => {
            let db = app.state::<DbState>();
            root_folder(&db, &client).await?
        }
    };
    let mut files = client.list_children(&parent, false).await?;
    files.sort_by(|a, b| {
        let af = a.mime_type == "application/vnd.google-apps.folder";
        let bf = b.mime_type == "application/vnd.google-apps.folder";
        bf.cmp(&af).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(files.into_iter().map(DriveEntry::from).collect())
}

/// Download a remote clip into its local game folder and index it.
#[tauri::command]
pub async fn drive_download(
    app: AppHandle,
    file_id: String,
    folder: Option<String>,
) -> Result<ClipRecord, String> {
    let client = super::drive_client_for(&app).await?;
    let meta = client.get_file(&file_id, "id,name,size").await?;
    let folder = folder
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| "Unknown".to_string());
    let db = app.state::<DbState>();
    let base = db.clips_dir()?;
    let dir = base.join(&folder);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("cannot create game folder {folder}: {e}"))?;
    let name = unique_local_name(&db, &folder, &meta.name);
    let dest = dir.join(&name);
    let emit_app = app.clone();
    let emit_id = file_id.clone();
    client
        .download_file(&file_id, &dest, move |sent, total| {
            let _ = emit_app.emit(
                "moonclip://download-progress",
                serde_json::json!({"fileId": emit_id, "sent": sent, "total": total}),
            );
        })
        .await?;

    // Index it like any other clip: probe + thumbnail + row.
    let ffmpeg = crate::editor::ffmpeg::resolve_ffmpeg(&app)?;
    let stem = dest
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("bad file name")?;
    let thumb_bare = format!("thumb_{stem}.jpg");
    let thumb_path = dir.join(&thumb_bare);
    let duration = crate::editor::ffmpeg::probe_duration_ms(&ffmpeg, &dest)
        .await
        .unwrap_or(0);
    if let Err(e) =
        crate::editor::ffmpeg::make_thumbnail(&ffmpeg, &dest, &thumb_path, 1.0).await
    {
        let _ = tokio::fs::remove_file(&dest).await;
        return Err(e);
    }
    let size = tokio::fs::metadata(&dest)
        .await
        .map_err(|e| format!("cannot stat the download: {e}"))?
        .len() as i64;
    let record = db.insert_clip(
        &format!("{folder}/{name}"),
        &format!("{folder}/{thumb_bare}"),
        &folder,
        duration,
        size,
        &folder,
    )?;
    let _ = db.enforce_quota(Some(&record.id), None);
    let _ = app.emit("moonclip://clip-saved", &record);
    crate::cue::play_ding();
    Ok(record)
}

fn unique_local_name(db: &DbState, folder: &str, name: &str) -> String {
    let taken: std::collections::HashSet<String> = db
        .list_clips()
        .map(|clips| clips.into_iter().map(|c| c.file_name).collect())
        .unwrap_or_default();
    if !taken.contains(&format!("{folder}/{name}")) {
        return name.to_string();
    }
    let (stem, ext) = split_name(name);
    for n in 2..1000 {
        let candidate = format!("{stem}_{n}{ext}");
        if !taken.contains(&format!("{folder}/{candidate}")) {
            return candidate;
        }
    }
    name.to_string()
}
