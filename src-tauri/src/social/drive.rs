//! Google Drive API client (drive.file scope: only files/folders MoonClip
//! creates). Resumable uploads and downloads land in the next commit; this
//! module owns folder discovery/creation so uploads have a mirrored home.

use serde::{Deserialize, Serialize};

use crate::storage::DbState;

const API: &str = "https://www.googleapis.com/drive/v3";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const ROOT_FOLDER_NAME: &str = "MoonClip";

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
    #[serde(default, rename = "thumbnailLink")]
    pub thumbnail_link: Option<String>,
}

#[derive(Deserialize)]
struct FileList {
    #[serde(default)]
    files: Vec<DriveFile>,
}

pub struct DriveClient {
    http: reqwest::Client,
    token: String,
    api: String,
}

impl DriveClient {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            token: token.into(),
            api: API.to_string(),
        }
    }

    /// Test hook: point the client at a local mock server.
    #[cfg(test)]
    fn with_base(base: String, token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            token: token.into(),
            api: base,
        }
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

    /// Files matching a Drive query, newest first.
    pub async fn list(&self, query: &str, fields: &str) -> Result<Vec<DriveFile>, String> {
        let url = format!(
            "{}/files?q={}&fields={}&pageSize=200&orderBy=modifiedTime desc",
            self.api,
            urlencoding::encode(query),
            urlencoding::encode(fields),
        );
        let value = self.get_json(&url).await?;
        let list: FileList =
            serde_json::from_value(value).map_err(|e| format!("unexpected file list: {e}"))?;
        Ok(list.files)
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
pub async fn ensure_root_folder(db: &DbState, access_token: &str) -> Result<String, String> {
    let client = DriveClient::new(access_token);
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
}
