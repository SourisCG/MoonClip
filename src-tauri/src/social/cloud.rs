//! Drive-only clips: on-demand downloads and cache lifecycle.
//!
//! A cloud clip has `cloud = 1` and `drive_file_id` set, its local video was
//! deleted after upload and only the thumbnail remains. Anything that needs
//! the actual file (preview, quick trim, heavy editor, export) goes through
//! `ensure_local`, which downloads it to the app cache on demand. The heavy
//! editor instead downloads into its session dir (wiped on close), so this
//! cache only serves the light paths.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter, Manager};

use crate::storage::models::ClipRecord;
use crate::storage::paths;

use super::drive::DriveClient;

/// `<app_cache>/cloud/<clip_id>/<bare file name>`
pub fn cache_path(app: &AppHandle, clip: &ClipRecord) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("no cache dir: {e}"))?
        .join("cloud")
        .join(&clip.id);
    let bare = clip
        .file_name
        .rsplit('/')
        .next()
        .ok_or("bad clip file name")?;
    Ok(dir.join(bare))
}

/// Local path of the clip's video, downloading it from Drive when needed.
pub async fn ensure_local(app: &AppHandle, clip: &ClipRecord) -> Result<PathBuf, String> {
    let base = {
        let db = app.state::<crate::storage::DbState>();
        db.clips_dir()?
    };
    let local = paths::resolve_clip_path(&base, &clip.file_name);
    if local.is_file() {
        return Ok(local);
    }
    if !clip.cloud {
        return Err(format!("clip file missing: {}", clip.file_name));
    }
    let file_id = clip
        .drive_file_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "cloud clip without a Drive id".to_string())?;
    let client = super::drive_client_for(app).await?;
    let dest = cache_path(app, clip)?;
    if dest.is_file() {
        return Ok(dest);
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("cannot create the cloud cache: {e}"))?;
    }
    download_with_progress(&client, file_id, &dest, app, &clip.id).await?;
    Ok(dest)
}

/// Shared by `ensure_local` and the editor session download.
pub async fn download_with_progress(
    client: &DriveClient,
    file_id: &str,
    dest: &Path,
    app: &AppHandle,
    clip_id: &str,
) -> Result<(), String> {
    let emit_app = app.clone();
    let emit_id = clip_id.to_string();
    client
        .download_file(file_id, dest, move |sent, total| {
            let _ = emit_app.emit(
                "moonclip://download-progress",
                serde_json::json!({"fileId": emit_id, "sent": sent, "total": total}),
            );
        })
        .await
}

/// Heavy editor download location: inside the session dir, wiped on close.
pub fn session_source_path(session_dir: &Path, clip_id: &str) -> PathBuf {
    session_dir.join(clip_id).join("source-download.mp4")
}

/// Delete the on-demand cache for a clip (light editor closes, clip removed).
pub fn cleanup_cache(app: &AppHandle, clip_id: &str) {
    let Ok(cache) = app.path().app_cache_dir() else {
        return;
    };
    let dir = cache.join("cloud").join(clip_id);
    if dir.is_dir() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Move the remote file to the Drive trash (recoverable), for cloud deletes.
pub async fn trash_remote(app: &AppHandle, clip: &ClipRecord) -> Result<(), String> {
    let file_id = clip
        .drive_file_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "cloud clip without a Drive id".to_string())?;
    let client = super::drive_client_for(app).await?;
    client.trash_file(file_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The download helper streams into the given path (mock Drive server).
    #[tokio::test]
    async fn download_helper_writes_the_file() {
        let dir = std::env::temp_dir().join(format!(
            "moonclip-cloud-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("clip.mp4");
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let handle = std::thread::spawn(move || {
            let request = server.recv().unwrap();
            assert!(request.url().contains("alt=media"));
            request
                .respond(tiny_http::Response::from_string("cloud-bytes"))
                .unwrap();
        });
        let client = DriveClient::with_base(format!("http://127.0.0.1:{port}"), "tok");
        client
            .download_file("f1", &dest, |_, _| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"cloud-bytes");
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
