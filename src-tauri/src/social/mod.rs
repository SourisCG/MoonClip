//! Social integrations (Phase 6): OAuth 2.0 loopback, OS-keyring token
//! storage and per-provider clients. All secrets stay in the keyring; the
//! only config in the clear is the public client id (+ the Google desktop
//! client secret, which Google does not treat as confidential).
//!
//! Config lives OUTSIDE the repository: `social.json` in the app data dir
//! (env vars win over the file). See `social.example.json`.

pub mod commands;
pub mod drive;
pub mod google;
pub mod oauth;
pub mod token_store;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::storage::DbState;

/// Per-provider public credentials.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClientCredentials {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
}

/// `social.json` contents. Every provider is optional: an absent provider is
/// simply shown as "not configured" in the UI.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SocialConfig {
    #[serde(default)]
    pub google_drive: Option<ClientCredentials>,
    #[serde(default)]
    pub google_youtube: Option<ClientCredentials>,
    #[serde(default)]
    pub tiktok: Option<ClientCredentials>,
    /// Cloudflare Worker that brokers the TikTok token exchange.
    #[serde(default)]
    pub tiktok_worker_url: Option<String>,
}

/// Load the config: environment overrides first, then `<app_data>/social.json`.
pub fn load_config(app: &AppHandle) -> SocialConfig {
    let mut config = SocialConfig::default();
    if let Ok(dir) = app.path().app_data_dir() {
        config = read_config_file(&dir.join("social.json"));
    }
    apply_env(&mut config);
    config
}

fn read_config_file(path: &PathBuf) -> SocialConfig {
    let Ok(text) = std::fs::read_to_string(path) else {
        return SocialConfig::default();
    };
    match serde_json::from_str::<SocialConfig>(&text) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("[moonclip] social.json is invalid ({e}); integrations disabled");
            SocialConfig::default()
        }
    }
}

fn apply_env(config: &mut SocialConfig) {
    let id = std::env::var("MOONCLIP_GOOGLE_DRIVE_CLIENT_ID").ok();
    let secret = std::env::var("MOONCLIP_GOOGLE_DRIVE_CLIENT_SECRET").unwrap_or_default();
    if let Some(client_id) = id.filter(|v| !v.trim().is_empty()) {
        config.google_drive = Some(ClientCredentials {
            client_id,
            client_secret: secret,
        });
    }
    if let Ok(url) = std::env::var("MOONCLIP_TIKTOK_WORKER_URL") {
        if !url.trim().is_empty() {
            config.tiktok_worker_url = Some(url);
        }
    }
}

/// One provider's state for the Accounts UI.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderStatus {
    /// Public client credentials present (social.json / env).
    pub configured: bool,
    /// Token blob present in the OS vault.
    pub connected: bool,
    /// Account label learned at connect time (email / display name).
    pub account: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SocialStatus {
    pub google_drive: ProviderStatus,
    pub google_youtube: ProviderStatus,
    pub tiktok: ProviderStatus,
}

fn provider_status(
    configured: bool,
    alias: &str,
) -> ProviderStatus {
    match token_store::load(alias) {
        Ok(Some(tokens)) => ProviderStatus {
            configured,
            connected: true,
            account: tokens.account,
        },
        Ok(None) => ProviderStatus {
            configured,
            connected: false,
            account: String::new(),
        },
        Err(e) => {
            eprintln!("[moonclip] cannot read the vault for {alias}: {e}");
            ProviderStatus {
                configured,
                connected: false,
                account: String::new(),
            }
        }
    }
}

#[tauri::command]
pub fn social_status(app: AppHandle) -> Result<SocialStatus, String> {
    let config = load_config(&app);
    Ok(SocialStatus {
        google_drive: provider_status(config.google_drive.is_some(), token_store::GOOGLE_DRIVE),
        google_youtube: provider_status(
            config.google_youtube.is_some(),
            token_store::GOOGLE_YOUTUBE,
        ),
        tiktok: provider_status(
            config.tiktok.is_some() && config.tiktok_worker_url.is_some(),
            token_store::TIKTOK,
        ),
    })
}

/// Connect Google Drive: browser consent + loopback callback + token vault +
/// root folder ("MoonClip") so uploads have a home.
#[tauri::command]
pub async fn connect_google_drive(app: AppHandle) -> Result<SocialStatus, String> {
    let config = load_config(&app);
    let client = config
        .google_drive
        .ok_or_else(|| "Google Drive is not configured (social.json)".to_string())?;
    google::connect(&app, google::Provider::Drive, &client).await?;
    // Round-trip through the vault: this is what uploads will use later.
    let access = google::access_token(google::Provider::Drive, &client).await?;
    {
        let db = app.state::<DbState>();
        let drive_client = drive::DriveClient::new(access);
        drive::ensure_root_folder(&db, &drive_client).await?;
    }
    social_status(app)
}

#[tauri::command]
pub async fn disconnect_google_drive(app: AppHandle) -> Result<SocialStatus, String> {
    token_store::delete(token_store::GOOGLE_DRIVE)?;
    {
        let db = app.state::<DbState>();
        db.clear_drive_folders()?;
        let _ = db.set_setting("drive_root_folder_id", "");
    }
    social_status(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parses_and_ignores_unknown_providers() {
        let config: SocialConfig = serde_json::from_str(
            r#"{
                "google_drive": {"client_id": "id", "client_secret": "sec"},
                "future_provider": {"x": 1}
            }"#,
        )
        .unwrap();
        assert_eq!(config.google_drive.unwrap().client_id, "id");
        assert!(config.google_youtube.is_none());
    }

    #[test]
    fn env_overrides_the_file_config() {
        // Serialize env access: this test owns these variables.
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("MOONCLIP_GOOGLE_DRIVE_CLIENT_ID", "env-id");
        std::env::set_var("MOONCLIP_GOOGLE_DRIVE_CLIENT_SECRET", "env-secret");
        let mut config = SocialConfig {
            google_drive: Some(ClientCredentials {
                client_id: "file-id".into(),
                client_secret: "file-secret".into(),
            }),
            ..Default::default()
        };
        apply_env(&mut config);
        let drive = config.google_drive.unwrap();
        assert_eq!(drive.client_id, "env-id");
        assert_eq!(drive.client_secret, "env-secret");
        std::env::remove_var("MOONCLIP_GOOGLE_DRIVE_CLIENT_ID");
        std::env::remove_var("MOONCLIP_GOOGLE_DRIVE_CLIENT_SECRET");
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
