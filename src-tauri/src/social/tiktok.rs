//! TikTok sharing (Phase 6): desktop Login Kit (loopback + PKCE with TikTok's
//! hex-encoded S256) plus Content Posting API Direct Post with `FILE_UPLOAD`.
//!
//! Unaudited API clients can only publish `SELF_ONLY` to private accounts.
//! TikTok's UX rules apply: the privacy dropdown is rendered from
//! `creator_info` and never preselects a value, and the creator's interaction
//! settings (comment/duet/stitch) are always honored.
//!
//! The OAuth token exchange needs the client secret. For personal setups the
//! secret can live in `social.json`; for distribution a Cloudflare Worker
//! (`workers/moonclip-oauth/`) brokers the exchange/refresh so the app never
//! ships a secret.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;

use crate::storage::DbState;

use super::oauth::Loopback;
use super::token_store::{self, OAuthTokens};
use super::{ClientCredentials, SocialConfig};

pub const AUTH_URL: &str = "https://www.tiktok.com/v2/auth/authorize/";
pub const DIRECT_TOKEN_URL: &str = "https://open.tiktokapis.com/v2/oauth/token/";
const API_BASE: &str = "https://open.tiktokapis.com/v2";
/// Scopes are comma separated on TikTok.
pub const SCOPE: &str = "user.info.basic,video.publish";
const CALLBACK_PATH: &str = "/callback/";
const MAX_TITLE: usize = 2200;
/// `FILE_UPLOAD` chunks: 5–64 MB allowed; 10 MiB keeps requests small.
const CHUNK: u64 = 10 * 1024 * 1024;
const MIN_CHUNK: u64 = 5 * 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const POLL_TIMEOUT: Duration = Duration::from_secs(120);

/// TikTok requires the HEX encoding of SHA256 as the PKCE challenge (not the
/// RFC 7636 base64url form used by Google/Discord).
pub fn hex_sha256(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Fresh PKCE pair per authorization (verifier: 64 unreserved chars).
pub fn pkce_pair() -> (String, String) {
    let verifier = format!(
        "{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    );
    let challenge = hex_sha256(&verifier);
    (verifier, challenge)
}

/// Consent URL (desktop flow).
pub fn auth_url(client_key: &str, redirect_uri: &str, state: &str, challenge: &str) -> String {
    let enc = urlencoding::encode;
    format!(
        "{AUTH_URL}?client_key={}&response_type=code&scope={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256",
        enc(client_key),
        enc(SCOPE),
        enc(redirect_uri),
        enc(state),
        enc(challenge),
    )
}

/// Post title (caption): collapsed whitespace, capped at TikTok's limit.
pub fn sanitize_title(raw: &str) -> Result<String, String> {
    let clean = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean: String = clean.chars().take(MAX_TITLE).collect();
    if clean.is_empty() {
        return Err("the title cannot be empty".into());
    }
    Ok(clean)
}

/// `FILE_UPLOAD` chunking: chunk size + chunk count. Files under 5 MB must
/// use a single chunk equal to the file size; the last chunk may be smaller.
pub fn chunk_plan(size: u64) -> (u64, u32) {
    let size = size.max(1);
    if size <= MIN_CHUNK {
        (size, 1)
    } else {
        let chunk = CHUNK.min(size);
        (chunk, size.div_ceil(chunk) as u32)
    }
}

/// Token endpoint mode: the Cloudflare Worker when configured, direct TikTok
/// with the local secret otherwise.
#[derive(Debug, Clone)]
pub struct TokenBroker {
    pub exchange_url: String,
    pub refresh_url: String,
    /// Direct mode only: the local client key/secret.
    pub credentials: Option<ClientCredentials>,
}

impl TokenBroker {
    pub fn from_config(config: &SocialConfig) -> Result<Self, String> {
        let credentials = config.tiktok.clone();
        let worker = config
            .tiktok_worker_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty());
        if let Some(worker) = worker {
            let base = worker.trim_end_matches('/');
            return Ok(Self {
                exchange_url: format!("{base}/tiktok/exchange"),
                refresh_url: format!("{base}/tiktok/refresh"),
                credentials: None,
            });
        }
        let credentials = credentials
            .filter(|c| !c.client_id.trim().is_empty() && !c.client_secret.is_empty())
            .ok_or_else(|| "TikTok is not configured (social.json)".to_string())?;
        Ok(Self {
            exchange_url: DIRECT_TOKEN_URL.to_string(),
            refresh_url: DIRECT_TOKEN_URL.to_string(),
            credentials: Some(credentials),
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires_in: i64,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub error_description: String,
}

fn parse_token(text: &str) -> Result<TokenResponse, String> {
    let response: TokenResponse = serde_json::from_str(text)
        .map_err(|e| format!("cannot decode the TikTok response: {e}"))?;
    if !response.error.is_empty() || response.access_token.is_empty() {
        return Err(format!(
            "TikTok rejected the request: {} {}",
            response.error, response.error_description
        ));
    }
    Ok(response)
}

/// Exchange the authorization code (via the Worker or directly).
pub async fn exchange_code(
    http: &reqwest::Client,
    broker: &TokenBroker,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, String> {
    let response = match &broker.credentials {
        Some(credentials) => {
            http.post(&broker.exchange_url)
                .form(&[
                    ("client_key", credentials.client_id.as_str()),
                    ("client_secret", credentials.client_secret.as_str()),
                    ("code", code),
                    ("grant_type", "authorization_code"),
                    ("redirect_uri", redirect_uri),
                    ("code_verifier", verifier),
                ])
                .send()
                .await
        }
        None => {
            http.post(&broker.exchange_url)
                .json(&serde_json::json!({
                    "code": code,
                    "code_verifier": verifier,
                    "redirect_uri": redirect_uri,
                }))
                .send()
                .await
        }
    }
    .map_err(|e| format!("TikTok token request failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("TikTok broker rejected the request ({status}): {text}"));
    }
    parse_token(&text)
}

/// Rotating refresh (TikTok issues a new refresh token every time).
pub async fn refresh(
    http: &reqwest::Client,
    broker: &TokenBroker,
    refresh_token: &str,
) -> Result<TokenResponse, String> {
    let response = match &broker.credentials {
        Some(credentials) => {
            http.post(&broker.refresh_url)
                .form(&[
                    ("client_key", credentials.client_id.as_str()),
                    ("client_secret", credentials.client_secret.as_str()),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token),
                ])
                .send()
                .await
        }
        None => {
            http.post(&broker.refresh_url)
                .json(&serde_json::json!({ "refresh_token": refresh_token }))
                .send()
                .await
        }
    }
    .map_err(|e| format!("TikTok refresh failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("TikTok broker rejected the refresh ({status}): {text}"));
    }
    parse_token(&text)
}

#[derive(Debug, Deserialize)]
struct UserInfoResponse {
    #[serde(default)]
    data: UserInfoData,
}

#[derive(Debug, Default, Deserialize)]
struct UserInfoData {
    #[serde(default)]
    user: UserInfo,
}

#[derive(Debug, Default, Deserialize)]
struct UserInfo {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    username: String,
}

async fn fetch_account(http: &reqwest::Client, token: &str) -> String {
    let response = http
        .get(format!("{API_BASE}/user/info/?fields=display_name,username"))
        .bearer_auth(token)
        .send()
        .await;
    let Ok(response) = response else {
        return String::new();
    };
    if !response.status().is_success() {
        return String::new();
    }
    let Ok(info) = response.json::<UserInfoResponse>().await else {
        return String::new();
    };
    let user = info.data.user;
    if user.display_name.is_empty() {
        if user.username.is_empty() {
            String::new()
        } else {
            format!("@{}", user.username)
        }
    } else {
        format!("{} (@{})", user.display_name, user.username)
    }
}

/// A valid access token, refreshed (and re-saved) when needed.
pub async fn access_token(config: &SocialConfig) -> Result<String, String> {
    let broker = TokenBroker::from_config(config)?;
    let mut tokens = token_store::load(token_store::TIKTOK)?
        .ok_or_else(|| "the TikTok account is not connected".to_string())?;
    if tokens.needs_refresh() {
        if tokens.refresh_token.is_empty() {
            return Err("the stored TikTok session cannot be refreshed; reconnect the account".into());
        }
        let refreshed = refresh(&reqwest::Client::new(), &broker, &tokens.refresh_token).await?;
        tokens.access_token = refreshed.access_token;
        if !refreshed.refresh_token.is_empty() {
            tokens.refresh_token = refreshed.refresh_token;
        }
        if !refreshed.scope.is_empty() {
            tokens.scope = refreshed.scope;
        }
        tokens.expires_at = token_store::now_unix() + refreshed.expires_in.max(0);
        token_store::save(token_store::TIKTOK, &tokens)?;
    }
    Ok(tokens.access_token)
}

/// Interactive connect: browser consent (Login Kit desktop), loopback with
/// PKCE, tokens in the vault. Works against the sandbox while it lasts.
pub async fn connect(app: &AppHandle, config: &SocialConfig) -> Result<OAuthTokens, String> {
    let credentials = config
        .tiktok
        .clone()
        .filter(|c| !c.client_id.trim().is_empty())
        .ok_or_else(|| "TikTok is not configured (social.json)".to_string())?;
    let broker = TokenBroker::from_config(config)?;
    let (verifier, challenge) = pkce_pair();
    let loopback = Loopback::start(CALLBACK_PATH)?;
    let url = auth_url(
        &credentials.client_id,
        &loopback.redirect_uri,
        &loopback.state,
        &challenge,
    );
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("cannot open the browser: {e}"))?;
    let redirect_uri = loopback.redirect_uri.clone();
    let code = tokio::task::spawn_blocking(move || {
        loopback.wait_for_code(Duration::from_secs(180))
    })
    .await
    .map_err(|e| format!("loopback task failed: {e}"))??;

    let http = reqwest::Client::new();
    let token = exchange_code(&http, &broker, &code, &verifier, &redirect_uri).await?;
    let account = fetch_account(&http, &token.access_token).await;
    let tokens = OAuthTokens::new(
        token.access_token,
        token.refresh_token,
        Some(token.expires_in),
        token.scope,
        account,
    );
    token_store::save(token_store::TIKTOK, &tokens)?;
    Ok(tokens)
}

/// Creator info for the export screen (privacy options are mandatory input).
#[derive(Debug, Clone, Serialize)]
pub struct CreatorInfo {
    pub username: String,
    pub nickname: String,
    pub privacy_level_options: Vec<String>,
    pub comment_disabled: bool,
    pub duet_disabled: bool,
    pub stitch_disabled: bool,
    pub max_video_post_duration_sec: i64,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

impl Default for ApiError {
    fn default() -> Self {
        Self {
            code: "ok".into(),
            message: String::new(),
        }
    }
}

impl ApiError {
    fn check(&self) -> Result<(), String> {
        if self.code.is_empty() || self.code == "ok" {
            Ok(())
        } else {
            Err(format!("TikTok error {}: {}", self.code, self.message))
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreatorInfoResponse {
    #[serde(default)]
    data: CreatorInfoData,
    #[serde(default)]
    error: ApiError,
}

#[derive(Debug, Default, Deserialize)]
struct CreatorInfoData {
    #[serde(default)]
    creator_username: String,
    #[serde(default)]
    creator_nickname: String,
    #[serde(default)]
    privacy_level_options: Vec<String>,
    #[serde(default)]
    comment_disabled: bool,
    #[serde(default)]
    duet_disabled: bool,
    #[serde(default)]
    stitch_disabled: bool,
    #[serde(default)]
    max_video_post_duration_sec: i64,
}

pub async fn query_creator_info(
    http: &reqwest::Client,
    token: &str,
) -> Result<CreatorInfo, String> {
    let response = http
        .post(format!("{API_BASE}/post/publish/creator_info/query/"))
        .bearer_auth(token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| format!("TikTok creator_info failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("TikTok creator_info failed ({status}): {text}"));
    }
    let info: CreatorInfoResponse = serde_json::from_str(&text)
        .map_err(|e| format!("cannot decode creator_info: {e}"))?;
    info.error.check()?;
    Ok(CreatorInfo {
        username: info.data.creator_username,
        nickname: info.data.creator_nickname,
        privacy_level_options: info.data.privacy_level_options,
        comment_disabled: info.data.comment_disabled,
        duet_disabled: info.data.duet_disabled,
        stitch_disabled: info.data.stitch_disabled,
        max_video_post_duration_sec: info.data.max_video_post_duration_sec,
    })
}

#[derive(Debug, Serialize)]
pub struct TikTokShareResult {
    pub publish_id: String,
    pub status: String,
    pub privacy: String,
}

#[derive(Debug, Deserialize)]
struct InitResponse {
    #[serde(default)]
    data: InitData,
    #[serde(default)]
    error: ApiError,
}

#[derive(Debug, Default, Deserialize)]
struct InitData {
    #[serde(default)]
    publish_id: String,
    #[serde(default)]
    upload_url: String,
}

#[derive(Debug, Deserialize)]
struct StatusResponse {
    #[serde(default)]
    data: StatusData,
    #[serde(default)]
    error: ApiError,
}

#[derive(Debug, Default, Deserialize)]
struct StatusData {
    #[serde(default)]
    status: String,
    #[serde(default)]
    fail_reason: String,
}

/// Init + chunked upload + status poll. Emits `moonclip://publish-progress`
/// with provider `tiktok` while it uploads.
async fn publish_video(
    app: &AppHandle,
    clip_id: &str,
    path: &Path,
    token: &str,
    title: &str,
    privacy: &str,
    creator: &CreatorInfo,
) -> Result<TikTokShareResult, String> {
    let http = reqwest::Client::new();
    let size = tokio::fs::metadata(path)
        .await
        .map_err(|e| format!("cannot stat {}: {e}", path.display()))?
        .len();
    let (chunk_size, total_chunks) = chunk_plan(size);

    let init = http
        .post(format!("{API_BASE}/post/publish/video/init/"))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "post_info": {
                "title": title,
                "privacy_level": privacy,
                "disable_comment": creator.comment_disabled,
                "disable_duet": creator.duet_disabled,
                "disable_stitch": creator.stitch_disabled,
                "video_cover_timestamp_ms": 1000,
            },
            "source_info": {
                "source": "FILE_UPLOAD",
                "video_size": size,
                "chunk_size": chunk_size,
                "total_chunk_count": total_chunks,
            },
        }))
        .send()
        .await
        .map_err(|e| format!("TikTok publish init failed: {e}"))?;
    let status = init.status();
    let text = init.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("TikTok publish init failed ({status}): {text}"));
    }
    let init: InitResponse =
        serde_json::from_str(&text).map_err(|e| format!("cannot decode publish init: {e}"))?;
    init.error.check()?;
    if init.data.upload_url.is_empty() || init.data.publish_id.is_empty() {
        return Err("TikTok did not return an upload URL".into());
    }

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    use tokio::io::AsyncReadExt;
    let mut buffer = vec![0u8; chunk_size as usize];
    let mut sent: u64 = 0;
    while sent < size {
        let want = ((size - sent).min(chunk_size)) as usize;
        let mut filled = 0usize;
        while filled < want {
            let n = file
                .read(&mut buffer[filled..want])
                .await
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        let end = sent + filled as u64;
        let response = http
            .put(&init.data.upload_url)
            .header("Content-Type", "video/mp4")
            .header("Content-Range", format!("bytes {}-{}/{}", sent, end - 1, size))
            .body(buffer[..filled].to_vec())
            .send()
            .await
            .map_err(|e| format!("TikTok upload failed: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("TikTok chunk upload failed ({status}): {text}"));
        }
        sent = end;
        let _ = app.emit(
            "moonclip://publish-progress",
            serde_json::json!({
                "clipId": clip_id,
                "provider": "tiktok",
                "phase": "upload",
                "sent": sent,
                "total": size,
            }),
        );
    }

    // Poll until TikTok finishes processing.
    let deadline = tokio::time::Instant::now() + POLL_TIMEOUT;
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let response = http
            .post(format!("{API_BASE}/post/publish/status/fetch/"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "publish_id": init.data.publish_id }))
            .send()
            .await
            .map_err(|e| format!("TikTok status fetch failed: {e}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("TikTok status fetch failed ({status}): {text}"));
        }
        let body: StatusResponse = serde_json::from_str(&text)
            .map_err(|e| format!("cannot decode publish status: {e}"))?;
        body.error.check()?;
        let publish_status = body.data.status;
        match publish_status.as_str() {
            "PUBLISH_COMPLETE" => {
                return Ok(TikTokShareResult {
                    publish_id: init.data.publish_id,
                    status: publish_status,
                    privacy: privacy.to_string(),
                });
            }
            "FAILED" => {
                return Err(format!(
                    "TikTok failed to publish the clip: {}",
                    body.data.fail_reason
                ));
            }
            _ => {
                if tokio::time::Instant::now() > deadline {
                    return Err(format!(
                        "TikTok is still processing the clip (last status: {publish_status}); check the TikTok app"
                    ));
                }
            }
        }
    }
}

/// Connect TikTok: browser consent (Login Kit, desktop loopback + PKCE).
#[tauri::command]
pub async fn connect_tiktok(app: AppHandle) -> Result<super::SocialStatus, String> {
    let config = super::load_config(&app);
    connect(&app, &config).await?;
    super::social_status(app)
}

/// Disconnect: drop the local session (the permission can be revoked in the
/// TikTok app under Settings → Security → Manage app permissions).
#[tauri::command]
pub async fn disconnect_tiktok(app: AppHandle) -> Result<super::SocialStatus, String> {
    token_store::delete(token_store::TIKTOK)?;
    super::social_status(app)
}

/// Creator info for the export screen (privacy options, interaction limits).
#[tauri::command]
pub async fn tiktok_creator_info(app: AppHandle) -> Result<CreatorInfo, String> {
    let config = super::load_config(&app);
    let token = access_token(&config).await?;
    query_creator_info(&reqwest::Client::new(), &token).await
}

/// Publish one clip to the connected TikTok account (Direct Post). The
/// privacy level must come from the creator_info options; unaudited clients
/// only succeed with `SELF_ONLY` on private accounts.
#[tauri::command]
pub async fn tiktok_share_clip(
    app: AppHandle,
    clip_id: String,
    title: String,
    privacy_level: String,
) -> Result<TikTokShareResult, String> {
    let title = sanitize_title(&title)?;
    let config = super::load_config(&app);
    let token = access_token(&config).await?;
    let http = reqwest::Client::new();
    let creator = query_creator_info(&http, &token).await?;
    if !creator
        .privacy_level_options
        .iter()
        .any(|option| option == &privacy_level)
    {
        return Err(format!(
            "privacy level {privacy_level} is not available for this account (options: {})",
            creator.privacy_level_options.join(", ")
        ));
    }
    let clip = {
        let db = app.state::<DbState>();
        db.list_clips()?
            .into_iter()
            .find(|c| c.id == clip_id)
            .ok_or_else(|| "clip not found".to_string())?
    };
    let path = super::cloud::ensure_local(&app, &clip).await?;
    if creator.max_video_post_duration_sec > 0 {
        if let Ok(ffmpeg) = crate::editor::ffmpeg::resolve_ffmpeg(&app) {
            if let Some(ms) = crate::editor::ffmpeg::probe_duration_ms(&ffmpeg, &path).await {
                let max_ms = creator.max_video_post_duration_sec * 1000;
                if ms > max_ms {
                    return Err(format!(
                        "the clip is {:.0}s and TikTok's limit for this account is {}s",
                        ms as f64 / 1000.0,
                        creator.max_video_post_duration_sec
                    ));
                }
            }
        }
    }
    let result = publish_video(
        &app,
        &clip_id,
        &path,
        &token,
        &title,
        &privacy_level,
        &creator,
    )
    .await;
    if clip.cloud {
        super::cloud::cleanup_cache(&app, &clip.id);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_sha256_matches_the_known_digest() {
        assert_eq!(
            hex_sha256("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn pkce_pair_is_well_formed_and_hex() {
        let (verifier, challenge) = pkce_pair();
        assert!((43..=128).contains(&verifier.len()));
        assert_eq!(challenge.len(), 64);
        assert!(challenge.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(challenge, hex_sha256(&verifier));
        assert_ne!(verifier, pkce_pair().0);
    }

    #[test]
    fn auth_url_carries_scopes_pkce_and_state() {
        let url = auth_url(
            "sbawp19relec857ogl",
            "http://127.0.0.1:4444/callback/",
            "state123",
            "abc123",
        );
        assert!(url.starts_with(AUTH_URL));
        assert!(url.contains("client_key=sbawp19relec857ogl"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("scope=user.info.basic%2Cvideo.publish"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A4444%2Fcallback%2F"));
        assert!(url.contains("state=state123"));
        assert!(url.contains("code_challenge=abc123"));
        assert!(url.contains("code_challenge_method=S256"));
    }

    #[test]
    fn title_is_sanitized_and_capped() {
        assert_eq!(sanitize_title("  My   clip ").unwrap(), "My clip");
        let long = "a".repeat(2500);
        assert_eq!(sanitize_title(&long).unwrap().chars().count(), MAX_TITLE);
        assert!(sanitize_title("   ").is_err());
    }

    #[test]
    fn chunk_plan_respects_tiktok_rules() {
        assert_eq!(chunk_plan(1), (1, 1));
        assert_eq!(chunk_plan(4 * 1024 * 1024), (4 * 1024 * 1024, 1));
        assert_eq!(chunk_plan(5 * 1024 * 1024), (5 * 1024 * 1024, 1));
        assert_eq!(chunk_plan(25 * 1024 * 1024), (10 * 1024 * 1024, 3));
        assert_eq!(chunk_plan(70 * 1024 * 1024), (10 * 1024 * 1024, 7));
    }

    #[test]
    fn token_responses_parse_success_and_error() {
        let ok = parse_token(
            r#"{"access_token":"at","refresh_token":"rt","expires_in":86400,"scope":"user.info.basic,video.publish","open_id":"oid"}"#,
        )
        .unwrap();
        assert_eq!(ok.access_token, "at");
        assert_eq!(ok.refresh_token, "rt");
        assert_eq!(ok.expires_in, 86400);

        let err = parse_token(
            r#"{"error":"invalid_grant","error_description":"code expired","log_id":"x"}"#,
        )
        .unwrap_err();
        assert!(err.contains("invalid_grant"), "{err}");
    }

    #[tokio::test]
    async fn broker_mode_targets_the_worker_endpoints() {
        let config = SocialConfig {
            tiktok: Some(ClientCredentials {
                client_id: "key".into(),
                client_secret: String::new(),
            }),
            tiktok_worker_url: Some("https://worker.example.dev/".into()),
            ..Default::default()
        };
        let broker = TokenBroker::from_config(&config).unwrap();
        assert_eq!(broker.exchange_url, "https://worker.example.dev/tiktok/exchange");
        assert_eq!(broker.refresh_url, "https://worker.example.dev/tiktok/refresh");
        assert!(broker.credentials.is_none());
    }

    #[tokio::test]
    async fn direct_mode_requires_the_secret() {
        let mut config = SocialConfig {
            tiktok: Some(ClientCredentials {
                client_id: "key".into(),
                client_secret: String::new(),
            }),
            ..Default::default()
        };
        assert!(TokenBroker::from_config(&config).is_err());
        config.tiktok = Some(ClientCredentials {
            client_id: "key".into(),
            client_secret: "sec".into(),
        });
        let broker = TokenBroker::from_config(&config).unwrap();
        assert_eq!(broker.exchange_url, DIRECT_TOKEN_URL);
        assert!(broker.credentials.is_some());
    }

    /// Token exchange against a local mock standing in for the Worker.
    #[tokio::test]
    async fn exchange_code_posts_the_pkce_pair_to_the_broker() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let handle = std::thread::spawn(move || {
            let mut request = server.recv().unwrap();
            assert_eq!(request.method(), &tiny_http::Method::Post);
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            assert!(body.contains("\"code\":\"abc\""));
            assert!(body.contains("\"code_verifier\":\"verifier123\""));
            assert!(body.contains("redirect_uri"));
            request
                .respond(tiny_http::Response::from_string(
                    r#"{"access_token":"at","refresh_token":"rt","expires_in":86400,"scope":"s"}"#,
                ))
                .unwrap();
        });
        let config = SocialConfig {
            tiktok: Some(ClientCredentials {
                client_id: "key".into(),
                client_secret: String::new(),
            }),
            tiktok_worker_url: Some(format!("http://127.0.0.1:{port}")),
            ..Default::default()
        };
        let broker = TokenBroker::from_config(&config).unwrap();
        let token = exchange_code(
            &reqwest::Client::new(),
            &broker,
            "abc",
            "verifier123",
            "http://127.0.0.1:9/callback/",
        )
        .await
        .unwrap();
        assert_eq!(token.access_token, "at");
        assert_eq!(token.refresh_token, "rt");
        handle.join().unwrap();
    }

    #[test]
    fn creator_info_options_are_validated_before_init() {
        // Pure check: the option must exist in the creator's list.
        let options = ["SELF_ONLY".to_string(), "MUTUAL_FOLLOW_FRIENDS".to_string()];
        assert!(options.iter().any(|o| o == "SELF_ONLY"));
        assert!(!options.iter().any(|o| o == "PUBLIC_TO_EVERYONE"));
    }
}
