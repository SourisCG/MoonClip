//! YouTube uploads (Phase 6): the same Google loopback OAuth as Drive plus a
//! resumable `videos.insert`. Upload-only by design — no browsing, no
//! downloads, nothing else is exposed in the app.
//!
//! Until the Google project passes the YouTube API compliance audit, every
//! upload made through the API is locked to private by YouTube itself (the
//! UI says so; the app cannot lift it).

use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::storage::DbState;

use super::google::{self, Provider};

/// Resumable session endpoint (`part` asks for the fields we read back).
const INIT_URL: &str =
    "https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status";
/// YouTube requires chunk sizes that are multiples of 256 KiB.
const CHUNK: usize = 8 * 1024 * 1024;
/// Fixed footer on every MoonClip upload (user decision: title + these tags).
const HASHTAGS: &str = "#MoonClip #moonclip";
/// Gaming: the closest category for gameplay clips.
const CATEGORY_GAMING: &str = "20";
/// YouTube's title limit.
const MAX_TITLE: usize = 100;

#[derive(Debug, Clone, Serialize)]
pub struct YouTubeUploadResult {
    pub video_id: String,
    pub url: String,
    pub privacy: String,
}

/// API privacy value; anything unknown stays private (safe default).
pub fn normalize_privacy(value: &str) -> &'static str {
    match value {
        "public" => "public",
        "unlisted" => "unlisted",
        _ => "private",
    }
}

/// YouTube forbids `<`/`>` in titles and caps them at 100 characters.
pub fn sanitize_title(raw: &str) -> Result<String, String> {
    let clean = raw
        .replace(['<', '>'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let clean: String = clean.chars().take(MAX_TITLE).collect();
    if clean.is_empty() {
        return Err("the title cannot be empty".into());
    }
    Ok(clean)
}

/// Metadata for the resumable session (pure: unit tested).
pub fn init_body(title: &str, privacy: &str) -> serde_json::Value {
    serde_json::json!({
        "snippet": {
            "title": title,
            "description": HASHTAGS,
            "categoryId": CATEGORY_GAMING,
        },
        "status": {
            "privacyStatus": privacy,
            "selfDeclaredMadeForKids": false,
        },
    })
}

/// Bytes acknowledged by a `Range: bytes=0-N` response header.
pub fn acked_bytes(range: Option<&str>) -> Option<u64> {
    range?
        .trim()
        .strip_prefix("bytes=0-")?
        .parse::<u64>()
        .ok()
        .map(|last| last + 1)
}

#[derive(Debug, Deserialize)]
struct VideoResource {
    #[serde(default)]
    id: String,
}

pub struct YouTubeClient {
    http: reqwest::Client,
    token: String,
    init_url: String,
}

impl YouTubeClient {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            token: token.into(),
            init_url: INIT_URL.to_string(),
        }
    }

    /// Test hook: point the client at a local mock server.
    #[cfg(test)]
    pub(crate) fn with_init_url(token: impl Into<String>, init_url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            token: token.into(),
            init_url,
        }
    }

    /// Resumable upload with progress. Mirrors the Drive client: retries a
    /// chunk once on transport errors and resumes from the server-reported
    /// range on 308 responses.
    pub async fn upload_file(
        &self,
        path: &Path,
        title: &str,
        privacy: &str,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<YouTubeUploadResult, String> {
        use tokio::io::AsyncReadExt;
        let total = tokio::fs::metadata(path)
            .await
            .map_err(|e| format!("cannot stat {}: {e}", path.display()))?
            .len();
        let session = self.initiate(title, privacy, total).await?;
        let mut file = tokio::fs::File::open(path)
            .await
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        let mut buf = vec![0u8; CHUNK];
        let mut sent = 0u64;
        while sent < total {
            let n = file
                .read(&mut buf)
                .await
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            if n == 0 {
                break;
            }
            let chunk_base = sent;
            let chunk_end = chunk_base + n as u64;
            let mut offset = 0usize;
            let mut retried = false;
            loop {
                let response = self
                    .http
                    .put(&session)
                    .bearer_auth(&self.token)
                    .header(
                        "Content-Range",
                        format!(
                            "bytes {}-{}/{total}",
                            chunk_base + offset as u64,
                            chunk_end - 1
                        ),
                    )
                    .body(buf[offset..n].to_vec())
                    .send()
                    .await;
                match response {
                    Ok(r) if r.status().as_u16() == 308 => {
                        // Resume from what the server actually received.
                        let acked = acked_bytes(
                            r.headers().get("range").and_then(|v| v.to_str().ok()),
                        )
                        .unwrap_or(chunk_end);
                        progress(acked.min(total), total);
                        if acked >= chunk_end {
                            sent = chunk_end;
                            break;
                        }
                        offset = (acked.saturating_sub(chunk_base)) as usize;
                    }
                    Ok(r) if r.status().is_success() => {
                        progress(total, total);
                        let text = r.text().await.unwrap_or_default();
                        let video: VideoResource = serde_json::from_str(&text)
                            .map_err(|e| format!("cannot decode the YouTube response: {e}"))?;
                        if video.id.is_empty() {
                            return Err("YouTube accepted the upload without an id".into());
                        }
                        return Ok(YouTubeUploadResult {
                            url: format!("https://youtu.be/{}", video.id),
                            video_id: video.id,
                            privacy: privacy.to_string(),
                        });
                    }
                    Ok(r) => {
                        let status = r.status();
                        let text = r.text().await.unwrap_or_default();
                        return Err(format!("upload failed ({status}): {text}"));
                    }
                    Err(e) if !retried => {
                        retried = true;
                        eprintln!("[moonclip] youtube upload chunk retry: {e}");
                    }
                    Err(e) => return Err(format!("upload failed: {e}")),
                }
            }
        }
        Err("upload finished without a YouTube response".into())
    }

    async fn initiate(&self, title: &str, privacy: &str, total: u64) -> Result<String, String> {
        let response = self
            .http
            .post(&self.init_url)
            .bearer_auth(&self.token)
            .header("X-Upload-Content-Type", "video/mp4")
            .header("X-Upload-Content-Length", total)
            .json(&init_body(title, privacy))
            .send()
            .await
            .map_err(|e| format!("YouTube request failed: {e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("cannot start the upload ({status}): {text}"));
        }
        response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| "YouTube did not return an upload session".to_string())
    }
}

/// Upload one clip to the connected YouTube channel. Upload-only: no local
/// state changes, no browsing; cloud clips download to the cache on demand
/// and the copy is dropped when the upload finishes.
#[tauri::command]
pub async fn youtube_share_clip(
    app: AppHandle,
    clip_id: String,
    title: String,
    privacy: String,
) -> Result<YouTubeUploadResult, String> {
    let title = sanitize_title(&title)?;
    let privacy = normalize_privacy(&privacy);
    let config = super::load_config(&app);
    let credentials = super::google_credentials(&config, Provider::YouTube)
        .ok_or_else(|| "YouTube is not configured (social.json)".to_string())?;
    let token = google::access_token(Provider::YouTube, &credentials).await?;
    let clip = {
        let db = app.state::<DbState>();
        db.list_clips()?
            .into_iter()
            .find(|c| c.id == clip_id)
            .ok_or_else(|| "clip not found".to_string())?
    };
    let path = super::cloud::ensure_local(&app, &clip).await?;
    let client = YouTubeClient::new(token);
    let emit_app = app.clone();
    let emit_id = clip_id.clone();
    let result = client
        .upload_file(&path, &title, privacy, move |sent, total| {
            let _ = emit_app.emit(
                "moonclip://publish-progress",
                serde_json::json!({
                    "clipId": emit_id,
                    "provider": "youtube",
                    "sent": sent,
                    "total": total,
                }),
            );
        })
        .await?;
    if clip.cloud {
        // The local copy was only the on-demand cache; the clip stays cloud.
        super::cloud::cleanup_cache(&app, &clip.id);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_is_validated_with_a_private_default() {
        assert_eq!(normalize_privacy("public"), "public");
        assert_eq!(normalize_privacy("unlisted"), "unlisted");
        assert_eq!(normalize_privacy("private"), "private");
        assert_eq!(normalize_privacy("PUBLIC"), "private");
        assert_eq!(normalize_privacy(""), "private");
    }

    #[test]
    fn title_is_sanitized_and_capped() {
        assert_eq!(sanitize_title("  My   clip  ").unwrap(), "My clip");
        assert_eq!(sanitize_title("<script>").unwrap(), "script");
        let long = "a".repeat(150);
        assert_eq!(sanitize_title(&long).unwrap().chars().count(), 100);
        assert!(sanitize_title("   ").is_err());
        assert!(sanitize_title("<>").is_err());
    }

    #[test]
    fn init_body_carries_title_tags_and_privacy() {
        let body = init_body("Overwatch ace", "unlisted");
        assert_eq!(body["snippet"]["title"], "Overwatch ace");
        assert_eq!(body["snippet"]["description"], "#MoonClip #moonclip");
        assert_eq!(body["snippet"]["categoryId"], "20");
        assert_eq!(body["status"]["privacyStatus"], "unlisted");
        assert_eq!(body["status"]["selfDeclaredMadeForKids"], false);
    }

    #[test]
    fn range_header_parsing() {
        assert_eq!(acked_bytes(Some("bytes=0-1023")), Some(1024));
        assert_eq!(acked_bytes(Some("bytes=0-0")), Some(1));
        assert_eq!(acked_bytes(Some("bytes=5-9")), None);
        assert_eq!(acked_bytes(None), None);
    }

    /// Live-ish: the resumable flow against a local mock (init -> 308 resume
    /// -> 200). Asserts the final result and the progress edges.
    #[tokio::test]
    async fn resumable_upload_resumes_and_finishes() {
        use std::sync::{Arc, Mutex};

        let dir = std::env::temp_dir().join(format!(
            "moonclip-youtube-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.mp4");
        let payload: Vec<u8> = (0..(300 * 1024u32)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&file, &payload).unwrap();

        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let session = format!("http://127.0.0.1:{port}/session");
        let session_for_thread = session.clone();
        let ranges: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let ranges_thread = ranges.clone();
        let handle = std::thread::spawn(move || {
            // 1) init: answer with the session URI.
            let mut request = server.recv().unwrap();
            assert_eq!(request.method(), &tiny_http::Method::Post);
            let mut body = String::new();
            let _ = request.as_reader().read_to_string(&mut body);
            assert!(body.contains("#MoonClip #moonclip"), "{body}");
            request
                .respond(
                    tiny_http::Response::empty(200).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Location"[..],
                            session_for_thread.as_bytes(),
                        )
                        .unwrap(),
                    ),
                )
                .unwrap();
            // 2) first chunk: acknowledge only half of it (resume path).
            let request = server.recv().unwrap();
            let range = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Content-Range"))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default();
            ranges_thread.lock().unwrap().push(range.clone());
            let total: u64 = range.rsplit('/').next().unwrap().parse().unwrap();
            let half = total / 2;
            request
                .respond(
                    tiny_http::Response::empty(308).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Range"[..],
                            format!("bytes=0-{}", half - 1).as_bytes(),
                        )
                        .unwrap(),
                    ),
                )
                .unwrap();
            // 3) the resumed tail: finish with the created video.
            let request = server.recv().unwrap();
            request
                .respond(tiny_http::Response::from_string(r#"{"id":"vid123"}"#))
                .unwrap();
        });

        let client = YouTubeClient::with_init_url("tok", format!("http://127.0.0.1:{port}/init"));
        let progress: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let progress_thread = progress.clone();
        let result = client
            .upload_file(&file, "Mock clip", "private", move |sent, total| {
                progress_thread.lock().unwrap().push((sent, total));
            })
            .await
            .unwrap();
        assert_eq!(result.video_id, "vid123");
        assert_eq!(result.url, "https://youtu.be/vid123");
        assert_eq!(result.privacy, "private");
        let seen = progress.lock().unwrap().clone();
        assert!(seen.iter().any(|(s, _)| *s > 0 && *s < payload.len() as u64));
        assert_eq!(seen.last(), Some(&(payload.len() as u64, payload.len() as u64)));
        let ranges = ranges.lock().unwrap().clone();
        assert!(ranges[0].starts_with("bytes 0-"), "{ranges:?}");
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
