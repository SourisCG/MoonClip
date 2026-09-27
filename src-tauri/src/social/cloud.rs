//! Drive-only clips: on-demand downloads and cache lifecycle.
//!
//! A cloud clip has `cloud = 1` and `drive_file_id` set, its local video was
//! deleted after upload and only the thumbnail remains. Anything that needs
//! the actual file (preview, quick trim, heavy editor, export) goes through
//! `ensure_local`, which downloads it to the app cache on demand. The heavy
//! editor instead downloads into its session dir (wiped on close), so this
//! cache only serves the light paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

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
    download_once(&client, file_id, &dest, |sent, total| {
        let _ = app.emit(
            "moonclip://download-progress",
            serde_json::json!({"fileId": clip.id, "sent": sent, "total": total}),
        );
    })
    .await?;
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
    download_once(client, file_id, dest, move |sent, total| {
        let _ = emit_app.emit(
            "moonclip://download-progress",
            serde_json::json!({"fileId": emit_id, "sent": sent, "total": total}),
        );
    })
    .await
}

/// Per-clip download locks: React StrictMode (dev) mounts effects twice, so
/// two concurrent calls would otherwise write the same file while the player
/// reads it (stutter + restarts).
fn clip_lock(key: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let map = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().expect("download locks poisoned");
    guard
        .entry(key.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// Single-flight, atomic download: the file is written to `dest.part` and
/// renamed when complete; a second caller waits and reuses the result.
pub(crate) async fn download_once(
    client: &DriveClient,
    file_id: &str,
    dest: &Path,
    progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    let lock = clip_lock(&dest.to_string_lossy());
    let _guard = lock.lock().await;
    if dest.is_file() {
        return Ok(());
    }
    let part = dest.with_extension("part");
    let _ = tokio::fs::remove_file(&part).await;
    client.download_file(file_id, &part, progress).await?;
    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| format!("cannot finalize the download: {e}"))?;
    Ok(())
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

    /// Two concurrent downloads of the same clip hit Drive ONCE and leave a
    /// complete file (single-flight + atomic rename).
    #[tokio::test]
    async fn concurrent_downloads_are_single_flight() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = std::env::temp_dir().join(format!(
            "moonclip-cloud-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("clip.mp4");
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_thread = hits.clone();
        let handle = std::thread::spawn(move || {
            // Serve up to 2 requests but only the first should ever happen.
            for _ in 0..2 {
                match server.recv_timeout(std::time::Duration::from_millis(1500)) {
                    Ok(Some(request)) => {
                        hits_thread.fetch_add(1, Ordering::SeqCst);
                        request
                            .respond(tiny_http::Response::from_string("cloud-bytes"))
                            .unwrap();
                    }
                    _ => break,
                }
            }
        });
        let client = Arc::new(DriveClient::with_base(
            format!("http://127.0.0.1:{port}"),
            "tok",
        ));
        let (a, b) = tokio::join!(
            download_once(&client, "f1", &dest, |_, _| {}),
            download_once(&client, "f1", &dest, |_, _| {}),
        );
        a.unwrap();
        b.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1, "Drive must be hit once");
        assert_eq!(std::fs::read(&dest).unwrap(), b"cloud-bytes");
        assert!(!dest.with_extension("part").exists());
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
