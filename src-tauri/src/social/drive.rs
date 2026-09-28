//! Google Drive API client (drive.file scope: only files/folders MoonClip
//! creates). Resumable uploads and downloads land in the next commit; this
//! module owns folder discovery/creation so uploads have a mirrored home.

use serde::{Deserialize, Serialize};

use crate::storage::DbState;

const API: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const ROOT_FOLDER_NAME: &str = "MoonClip";
/// Resumable upload chunk (Google requires multiples of 256 KiB).
const CHUNK: usize = 8 * 1024 * 1024;

/// Video metadata Drive exposes for uploaded videos (duration for restored
/// library rows).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VideoMediaMetadata {
    #[serde(default, rename = "durationMillis")]
    pub duration_millis: Option<i64>,
    #[serde(default)]
    pub width: Option<i64>,
    #[serde(default)]
    pub height: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveFile {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "mimeType")]
    pub mime_type: String,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default, rename = "modifiedTime")]
    pub modified_time: Option<String>,
    #[serde(default, rename = "createdTime")]
    pub created_time: Option<String>,
    #[serde(default, rename = "thumbnailLink")]
    pub thumbnail_link: Option<String>,
    #[serde(default, rename = "webViewLink")]
    pub web_view_link: Option<String>,
    #[serde(default, rename = "videoMediaMetadata")]
    pub video_media_metadata: Option<VideoMediaMetadata>,
}

#[derive(Deserialize)]
struct FileList {
    #[serde(default)]
    files: Vec<DriveFile>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

pub struct DriveClient {
    http: reqwest::Client,
    token: String,
    api: String,
    upload_api: String,
}

impl DriveClient {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            token: token.into(),
            api: API.to_string(),
            upload_api: UPLOAD_API.to_string(),
        }
    }

    /// Test hook: point the client at a local mock server.
    #[cfg(test)]
    pub(crate) fn with_base(base: String, token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            token: token.into(),
            api: base.clone(),
            upload_api: base,
        }
    }

    /// The file when it still exists and is not trashed; `None` when gone.
    pub async fn live_file(&self, file_id: &str) -> Result<Option<DriveFile>, String> {
        let response = self
            .http
            .get(format!(
                "{}/files/{}?fields=id,name,mimeType,size,modifiedTime,trashed",
                self.api,
                urlencoding::encode(file_id)
            ))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| format!("Drive request failed: {e}"))?;
        match response.status().as_u16() {
            404 => Ok(None),
            status if (200..300).contains(&status) => {
                let text = response.text().await.unwrap_or_default();
                let value: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|e| format!("cannot decode the Drive file: {e}"))?;
                if value
                    .get("trashed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    return Ok(None);
                }
                serde_json::from_value(value)
                    .map(Some)
                    .map_err(|e| format!("unexpected Drive file: {e}"))
            }
            status => {
                let text = response.text().await.unwrap_or_default();
                Err(format!("Drive rejected the request ({status}): {text}"))
            }
        }
    }

    /// Raw fields (webViewLink and friends are not in DriveFile).
    pub async fn raw_file(
        &self,
        file_id: &str,
        fields: &str,
    ) -> Result<serde_json::Value, String> {
        let url = format!(
            "{}/files/{}?fields={}",
            self.api,
            urlencoding::encode(file_id),
            urlencoding::encode(fields),
        );
        self.get_json(&url).await
    }

    pub async fn get_file(&self, file_id: &str, fields: &str) -> Result<DriveFile, String> {
        let url = format!(
            "{}/files/{}?fields={}",
            self.api,
            urlencoding::encode(file_id),
            urlencoding::encode(fields),
        );
        let value = self.get_json(&url).await?;
        serde_json::from_value(value).map_err(|e| format!("unexpected file: {e}"))
    }

    /// Direct children of a folder (optionally folders only).
    pub async fn list_children(
        &self,
        parent: &str,
        folders_only: bool,
    ) -> Result<Vec<DriveFile>, String> {
        let escaped = parent.replace('\'', "\\'");
        let mut query = format!("'{escaped}' in parents and trashed = false");
        if folders_only {
            query.push_str(&format!(" and mimeType = '{FOLDER_MIME}'"));
        }
        self.list(
            &query,
            "files(id,name,mimeType,size,modifiedTime,thumbnailLink)",
        )
        .await
    }

    /// First file with this exact name inside `parent` (for dedupe).
    pub async fn find_file(
        &self,
        name: &str,
        parent: &str,
    ) -> Result<Option<DriveFile>, String> {
        let escaped = name.replace('\'', "\\'");
        let query = format!(
            "name = '{escaped}' and '{parent}' in parents and trashed = false"
        );
        let files = self
            .list(&query, "files(id,name,mimeType,size,modifiedTime)")
            .await?;
        Ok(files.into_iter().next())
    }

    /// Move the file to the Drive trash (recoverable, unlike a hard delete).
    pub async fn trash_file(&self, file_id: &str) -> Result<(), String> {
        let response = self
            .http
            .patch(format!("{}/files/{}", self.api, file_id))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({"trashed": true}))
            .send()
            .await
            .map_err(|e| format!("Drive request failed: {e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("cannot trash the Drive file ({status}): {text}"));
        }
        Ok(())
    }

    /// Anyone-with-the-link read access (Medal-style public link).
    pub async fn set_public(&self, file_id: &str) -> Result<(), String> {
        let response = self
            .http
            .post(format!("{}/files/{}/permissions", self.api, file_id))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({"role": "reader", "type": "anyone"}))
            .send()
            .await
            .map_err(|e| format!("Drive request failed: {e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("cannot make the file public ({status}): {text}"));
        }
        Ok(())
    }

    /// Resumable upload with progress. Retries a chunk once on transport
    /// errors and resumes from the server-reported range on 308 responses.
    pub async fn upload_file(
        &self,
        path: &std::path::Path,
        name: &str,
        parent: &str,
        mime: &str,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<DriveFile, String> {
        use tokio::io::AsyncReadExt;
        let total = tokio::fs::metadata(path)
            .await
            .map_err(|e| format!("cannot stat {}: {e}", path.display()))?
            .len();
        let session = self.initiate_upload(name, parent, mime, total).await?;
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
                        format!("bytes {}-{}/{total}", chunk_base + offset as u64, chunk_end - 1),
                    )
                    .body(buf[offset..n].to_vec())
                    .send()
                    .await;
                match response {
                    Ok(r) if r.status().as_u16() == 308 => {
                        // Resume from what the server actually received: the
                        // unsent tail of THIS chunk is resent with a new range.
                        let acked = r
                            .headers()
                            .get("range")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.strip_prefix("bytes=0-"))
                            .and_then(|v| v.parse::<u64>().ok())
                            .map(|last| last + 1)
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
                        return serde_json::from_str(&text)
                            .map_err(|e| format!("cannot decode the uploaded file: {e}"));
                    }
                    Ok(r) => {
                        let status = r.status();
                        let text = r.text().await.unwrap_or_default();
                        return Err(format!("upload failed ({status}): {text}"));
                    }
                    Err(e) if !retried => {
                        retried = true;
                        eprintln!("[moonclip] upload chunk retry: {e}");
                    }
                    Err(e) => return Err(format!("upload failed: {e}")),
                }
            }
        }
        Err("upload finished without a Drive response".into())
    }

    async fn initiate_upload(
        &self,
        name: &str,
        parent: &str,
        mime: &str,
        total: u64,
    ) -> Result<String, String> {
        let response = self
            .http
            .post(format!(
                "{}/files?uploadType=resumable&fields=id,name,mimeType,size,modifiedTime",
                self.upload_api
            ))
            .bearer_auth(&self.token)
            .header("X-Upload-Content-Type", mime)
            .header("X-Upload-Content-Length", total)
            .json(&serde_json::json!({"name": name, "parents": [parent]}))
            .send()
            .await
            .map_err(|e| format!("Drive request failed: {e}"))?;
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
            .ok_or_else(|| "Drive did not return an upload session".to_string())
    }

    /// Stream a file to `dest` with progress.
    pub async fn download_file(
        &self,
        file_id: &str,
        dest: &std::path::Path,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<(), String> {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;
        let response = self
            .http
            .get(format!("{}/files/{}?alt=media", self.api, file_id))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| format!("Drive request failed: {e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("download failed ({status}): {text}"));
        }
        let total = response.content_length().unwrap_or(0);
        let mut out = tokio::fs::File::create(dest)
            .await
            .map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
        let mut stream = response.bytes_stream();
        let mut sent = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("download interrupted: {e}"))?;
            out.write_all(&chunk)
                .await
                .map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
            sent += chunk.len() as u64;
            progress(sent, total);
        }
        out.flush()
            .await
            .map_err(|e| format!("cannot flush {}: {e}", dest.display()))?;
        Ok(())
    }

    async fn get_json(&self, url: &str) -> Result<serde_json::Value, String> {
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| format!("Drive request failed: {e}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("Drive rejected the request ({status}): {text}"));
        }
        serde_json::from_str(&text).map_err(|e| format!("cannot decode the Drive response: {e}"))
    }

    /// Files matching a Drive query, newest first. Follows `nextPageToken`
    /// until the listing is complete (libraries can exceed 200 files).
    pub async fn list(&self, query: &str, fields: &str) -> Result<Vec<DriveFile>, String> {
        let mut files = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut url = format!(
                "{}/files?q={}&fields={}&pageSize=200&orderBy=modifiedTime desc",
                self.api,
                urlencoding::encode(query),
                urlencoding::encode(fields),
            );
            if let Some(token) = &token {
                url.push_str(&format!("&pageToken={}", urlencoding::encode(token)));
            }
            let value = self.get_json(&url).await?;
            let page: FileList = serde_json::from_value(value)
                .map_err(|e| format!("unexpected file list: {e}"))?;
            files.extend(page.files);
            match page.next_page_token {
                Some(next) if !next.is_empty() => token = Some(next),
                _ => break,
            }
        }
        Ok(files)
    }

    /// Children of a folder with everything the library sync needs (original
    /// timestamps, duration and the generated thumbnail link).
    pub async fn list_library_children(&self, parent: &str) -> Result<Vec<DriveFile>, String> {
        let escaped = parent.replace('\'', "\\'");
        let query = format!("'{escaped}' in parents and trashed = false");
        self.list(
            &query,
            "files(id,name,mimeType,size,modifiedTime,createdTime,thumbnailLink,webViewLink,videoMediaMetadata),nextPageToken",
        )
        .await
    }

    pub async fn find_folder(
        &self,
        name: &str,
        parent: Option<&str>,
    ) -> Result<Option<DriveFile>, String> {
        let escaped = name.replace('\'', "\\'");
        let mut query = format!(
            "name = '{escaped}' and mimeType = '{FOLDER_MIME}' and trashed = false"
        );
        if let Some(parent) = parent {
            query.push_str(&format!(" and '{parent}' in parents"));
        }
        let files = self
            .list(&query, "files(id,name,mimeType,modifiedTime)")
            .await?;
        Ok(files.into_iter().next())
    }

    pub async fn create_folder(
        &self,
        name: &str,
        parent: Option<&str>,
    ) -> Result<DriveFile, String> {
        let mut metadata = serde_json::json!({ "name": name, "mimeType": FOLDER_MIME });
        if let Some(parent) = parent {
            metadata["parents"] = serde_json::json!([parent]);
        }
        let response = self
            .http
            .post(format!("{}/files?fields=id,name,mimeType", self.api))
            .bearer_auth(&self.token)
            .json(&metadata)
            .send()
            .await
            .map_err(|e| format!("Drive request failed: {e}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("cannot create the Drive folder ({status}): {text}"));
        }
        serde_json::from_str(&text).map_err(|e| format!("cannot decode the created folder: {e}"))
    }

    /// Existing folder (case-sensitive, like Drive) or a fresh one.
    pub async fn ensure_folder(
        &self,
        name: &str,
        parent: Option<&str>,
    ) -> Result<DriveFile, String> {
        if let Some(found) = self.find_folder(name, parent).await? {
            return Ok(found);
        }
        self.create_folder(name, parent).await
    }

    pub async fn root_folder_id(&self) -> Result<String, String> {
        Ok(self
            .ensure_folder(ROOT_FOLDER_NAME, None)
            .await?
            .id)
    }
}

/// Ensure the `MoonClip` root folder exists and persist its id.
pub async fn ensure_root_folder(db: &DbState, client: &DriveClient) -> Result<String, String> {
    let id = client.root_folder_id().await?;
    db.set_setting("drive_root_folder_id", &id)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_mime_is_the_drive_folder_type() {
        assert_eq!(FOLDER_MIME, "application/vnd.google-apps.folder");
    }

    #[test]
    fn library_files_parse_the_sync_metadata() {
        let file: DriveFile = serde_json::from_str(
            r#"{
                "id": "f1",
                "name": "clip.mp4",
                "mimeType": "video/mp4",
                "size": "19654086",
                "createdTime": "2026-09-27T18:04:05.123Z",
                "thumbnailLink": "https://lh3.googleusercontent.com/x=s220",
                "webViewLink": "https://drive.google.com/file/d/f1/view",
                "videoMediaMetadata": {"durationMillis": 8800, "width": 1920, "height": 1080}
            }"#,
        )
        .unwrap();
        assert_eq!(file.created_time.as_deref(), Some("2026-09-27T18:04:05.123Z"));
        assert_eq!(file.thumbnail_link.as_deref(), Some("https://lh3.googleusercontent.com/x=s220"));
        assert_eq!(
            file.web_view_link.as_deref(),
            Some("https://drive.google.com/file/d/f1/view")
        );
        let metadata = file.video_media_metadata.unwrap();
        assert_eq!(metadata.duration_millis, Some(8800));
        assert_eq!(metadata.width, Some(1920));
    }

    /// `list` follows `nextPageToken` until the listing is complete.
    #[tokio::test]
    async fn list_follows_pagination() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let handle = std::thread::spawn(move || {
            let first = server.recv().unwrap();
            assert!(!first.url().contains("pageToken"));
            first
                .respond(tiny_http::Response::from_string(
                    r#"{"files":[{"id":"a","name":"a.mp4","mimeType":"video/mp4"}],"nextPageToken":"TOKEN1"}"#,
                ))
                .unwrap();
            let second = server.recv().unwrap();
            assert!(second.url().contains("pageToken=TOKEN1"), "{}", second.url());
            second
                .respond(tiny_http::Response::from_string(
                    r#"{"files":[{"id":"b","name":"b.mp4","mimeType":"video/mp4"}]}"#,
                ))
                .unwrap();
        });
        let client = DriveClient::with_base(format!("http://127.0.0.1:{port}"), "tok");
        let files = client
            .list("'root' in parents", "files(id,name,mimeType)")
            .await
            .unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].id, "a");
        assert_eq!(files[1].id, "b");
        handle.join().unwrap();
    }

    /// A local mock Drive answers the folder query and the create call.
    #[tokio::test]
    async fn ensure_folder_finds_or_creates() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr());
        let handle = std::thread::spawn(move || {
            // 1st call: list -> empty; 2nd: create -> folder JSON.
            let list = server.recv().unwrap();
            assert!(list.url().starts_with("/files?q="));
            list.respond(tiny_http::Response::from_string(r#"{"files":[]}"#))
                .unwrap();
            let create = server.recv().unwrap();
            create
                .respond(tiny_http::Response::from_string(
                    r#"{"id":"folder-1","name":"MoonClip","mimeType":"application/vnd.google-apps.folder"}"#,
                ))
                .unwrap();
        });
        let client = DriveClient::with_base(base, "tok");
        let folder = client.ensure_folder("MoonClip", None).await.unwrap();
        assert_eq!(folder.id, "folder-1");
        handle.join().unwrap();
    }

    /// A partial 308 must resume exactly where the server stopped, not drop
    /// the rest of the chunk.
    #[tokio::test]
    async fn upload_resumes_from_the_server_reported_range() {
        let dir = std::env::temp_dir().join(format!(
            "moonclip-upload-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.mp4");
        std::fs::write(&file, b"0123456789ab").unwrap(); // 12 bytes

        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let handle = std::thread::spawn(move || {
            // 1) initiate -> 200 + Location
            let init = server.recv().unwrap();
            assert_eq!(init.method(), &tiny_http::Method::Post);
            init.respond(
                tiny_http::Response::empty(200).with_header(
                    tiny_http::Header::from_bytes(
                        &b"Location"[..],
                        format!("http://127.0.0.1:{port}/session").into_bytes(),
                    )
                    .unwrap(),
                ),
            )
            .unwrap();
            // 2) first PUT: only 4 bytes stored -> 308 with Range
            let mut first = server.recv().unwrap();
            let mut body = Vec::new();
            first.as_reader().read_to_end(&mut body).unwrap();
            assert_eq!(body, b"0123456789ab");
            first
                .respond(
                    tiny_http::Response::empty(308)
                        .with_header(
                            tiny_http::Header::from_bytes(&b"Range"[..], &b"bytes=0-3"[..])
                                .unwrap(),
                        ),
                )
                .unwrap();
            // 3) second PUT: must resume at byte 4 with the tail
            let mut second = server.recv().unwrap();
            let range = second
                .headers()
                .iter()
                .find(|h| h.field.equiv("Content-Range"))
                .map(|h| h.value.as_str().to_string())
                .unwrap();
            assert_eq!(range, "bytes 4-11/12");
            let mut tail = Vec::new();
            second.as_reader().read_to_end(&mut tail).unwrap();
            assert_eq!(tail, b"456789ab");
            second
                .respond(tiny_http::Response::from_string(
                    r#"{"id":"f1","name":"clip.mp4","mimeType":"video/mp4"}"#,
                ))
                .unwrap();
        });
        let client = DriveClient::with_base(format!("http://127.0.0.1:{port}"), "tok");
        let uploaded = client
            .upload_file(&file, "clip.mp4", "parent-1", "video/mp4", |_, _| {})
            .await
            .unwrap();
        assert_eq!(uploaded.id, "f1");
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn live_file_reports_gone_and_trashed_as_none() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let handle = std::thread::spawn(move || {
            // 200 alive
            let alive = server.recv().unwrap();
            alive
                .respond(tiny_http::Response::from_string(
                    r#"{"id":"f1","name":"clip.mp4","mimeType":"video/mp4","trashed":false}"#,
                ))
                .unwrap();
            // 200 but trashed
            let trashed = server.recv().unwrap();
            trashed
                .respond(tiny_http::Response::from_string(
                    r#"{"id":"f2","name":"old.mp4","mimeType":"video/mp4","trashed":true}"#,
                ))
                .unwrap();
            // 404
            let gone = server.recv().unwrap();
            gone.respond(tiny_http::Response::empty(404)).unwrap();
        });
        let client = DriveClient::with_base(format!("http://127.0.0.1:{port}"), "tok");
        assert!(client.live_file("f1").await.unwrap().is_some());
        assert!(client.live_file("f2").await.unwrap().is_none());
        assert!(client.live_file("f3").await.unwrap().is_none());
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn trash_marks_the_file_trashed() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let handle = std::thread::spawn(move || {
            let mut request = server.recv().unwrap();
            assert_eq!(request.method(), &tiny_http::Method::Patch);
            assert_eq!(request.url(), "/files/f1");
            let mut body = Vec::new();
            request.as_reader().read_to_end(&mut body).unwrap();
            assert_eq!(String::from_utf8_lossy(&body), r#"{"trashed":true}"#);
            request.respond(tiny_http::Response::empty(200)).unwrap();
        });
        let client = DriveClient::with_base(format!("http://127.0.0.1:{port}"), "tok");
        client.trash_file("f1").await.unwrap();
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn download_streams_the_body_to_disk() {
        let dir = std::env::temp_dir().join(format!(
            "moonclip-download-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("out.mp4");
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let handle = std::thread::spawn(move || {
            let request = server.recv().unwrap();
            assert!(request.url().contains("alt=media"));
            request
                .respond(tiny_http::Response::from_string("hello-drive"))
                .unwrap();
        });
        let client = DriveClient::with_base(format!("http://127.0.0.1:{port}"), "tok");
        client
            .download_file("file-1", &dest, |_, _| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello-drive");
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
