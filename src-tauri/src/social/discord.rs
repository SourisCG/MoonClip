//! Discord sharing (Phase 6): "connect your account" instead of asking the
//! user to create a webhook by hand. Discord's `webhook.incoming` OAuth flow
//! lets the user pick a server + channel in the browser and returns a ready
//! webhook in the token response; the app stores its id/token in the OS
//! keyring and posts clips with a multipart upload.
//!
//! Public Client + PKCE S256: no client secret is needed or stored (verified
//! against Discord's OAuth2 docs), and the OAuth access token is revoked as
//! soon as the webhook has been captured.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::storage::{secrets, DbState};

use super::oauth::{Loopback, Pkce};

pub const AUTH_URL: &str = "https://discord.com/oauth2/authorize";
pub const TOKEN_URL: &str = "https://discord.com/api/oauth2/token";
pub const REVOKE_URL: &str = "https://discord.com/api/oauth2/token/revoke";
const API_BASE: &str = "https://discord.com/api/v10";
/// Registered in the Developer Portal; Discord matches it byte for byte.
pub const REDIRECT_URI: &str = "http://127.0.0.1:38471/callback";
const LOOPBACK_PORT: u16 = 38471;
/// `webhook.incoming` creates the webhook; `identify`/`guilds` only label it.
pub const SCOPES: &str = "webhook.incoming identify guilds";
/// MoonClip's Discord application (public client id, not a secret). Override
/// with `discord.client_id` in social.json or `MOONCLIP_DISCORD_CLIENT_ID`.
pub const DEFAULT_CLIENT_ID: &str = "1553938269874557059";
/// Keyring alias for the captured webhook.
pub const ALIAS: &str = "discord_webhook";
/// Discord's default (non-boosted) attachment limit.
pub const DEFAULT_MAX_MB: u64 = 10;
const MAX_TITLE: usize = 100;
const HASHTAGS: &str = "#MoonClip #moonclip";
const BOUNDARY: &str = "----MoonClipFormBoundary9f2c";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEntry {
    pub url: String,
    pub id: String,
    pub token: String,
    #[serde(default)]
    pub guild_id: String,
    #[serde(default)]
    pub channel_id: String,
    /// "@user · Guild · #channel" label for the Accounts row.
    #[serde(default)]
    pub account: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscordShareResult {
    pub message_id: String,
    /// Jump link, present when the guild id is known.
    pub url: Option<String>,
    pub channel: String,
}

pub fn load_webhook() -> Result<Option<WebhookEntry>, String> {
    match secrets::get_secret_opt(ALIAS)? {
        Some(text) => match serde_json::from_str::<WebhookEntry>(&text) {
            Ok(entry) => Ok(Some(entry)),
            Err(e) => {
                eprintln!("[moonclip] stored Discord webhook is unreadable: {e}");
                Ok(None)
            }
        },
        None => Ok(None),
    }
}

pub fn save_webhook(entry: &WebhookEntry) -> Result<(), String> {
    let text =
        serde_json::to_string(entry).map_err(|e| format!("cannot encode the webhook: {e}"))?;
    secrets::store_secret(ALIAS, &text)
}

pub fn delete_webhook() -> Result<(), String> {
    secrets::delete_secret(ALIAS)
}

/// Consent URL: PKCE S256 + state, no client secret (Public Client).
pub fn auth_url(client_id: &str, state: &str, challenge: &str) -> String {
    let enc = urlencoding::encode;
    format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&code_challenge={}&code_challenge_method=S256&prompt=consent",
        enc(client_id),
        enc(REDIRECT_URI),
        enc(SCOPES),
        enc(state),
        enc(challenge),
    )
}

/// Discord caps the message title like every other provider.
pub fn sanitize_title(raw: &str) -> Result<String, String> {
    let clean = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean: String = clean.chars().take(MAX_TITLE).collect();
    if clean.is_empty() {
        return Err("the title cannot be empty".into());
    }
    Ok(clean)
}

/// Webhook message payload: title + the fixed MoonClip tags. Mentions are
/// disabled so a title can never ping a role or everyone.
pub fn payload_json(title: &str) -> serde_json::Value {
    serde_json::json!({
        "content": format!("{title} {HASHTAGS}"),
        "allowed_mentions": { "parse": [] },
    })
}

/// Attachment limit for this install: `discord_max_mb` setting (10/25/50/100).
pub fn max_bytes(app: &AppHandle) -> u64 {
    let mb = app
        .state::<DbState>()
        .get_settings()
        .ok()
        .and_then(|s| s.get("discord_max_mb").and_then(|v| v.parse::<u64>().ok()))
        .unwrap_or(DEFAULT_MAX_MB)
        .clamp(1, 500);
    mb * 1024 * 1024
}

/// Pure size gate (unit tested).
pub fn fits(size: u64, limit: u64) -> bool {
    size <= limit
}

/// Preamble (payload_json part + file headers) and epilogue of the multipart
/// body. The file itself streams between them.
pub fn multipart_parts(payload: &str, filename: &str) -> (Vec<u8>, Vec<u8>) {
    let safe = filename.replace(['"', '\r', '\n'], "_");
    let preamble = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"payload_json\"\r\nContent-Type: application/json\r\n\r\n{payload}\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"files[0]\"; filename=\"{safe}\"\r\nContent-Type: video/mp4\r\n\r\n"
    );
    let epilogue = format!("\r\n--{BOUNDARY}--\r\n");
    (preamble.into_bytes(), epilogue.into_bytes())
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    webhook: Option<WebhookResource>,
}

#[derive(Debug, Deserialize)]
struct WebhookResource {
    id: String,
    token: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    guild_id: String,
    #[serde(default)]
    channel_id: String,
}

impl WebhookResource {
    fn into_entry(self) -> WebhookEntry {
        let url = if self.url.is_empty() {
            format!("{API_BASE}/webhooks/{}/{}", self.id, self.token)
        } else {
            self.url
        };
        WebhookEntry {
            url,
            id: self.id,
            token: self.token,
            guild_id: self.guild_id,
            channel_id: self.channel_id,
            account: String::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct MessageResource {
    #[serde(default)]
    id: String,
    #[serde(default)]
    channel_id: String,
}

#[derive(Debug, Deserialize)]
struct UserResource {
    #[serde(default)]
    username: String,
    #[serde(default)]
    global_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GuildResource {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct ChannelResource {
    #[serde(default)]
    name: String,
}

async fn get_json<T: for<'de> Deserialize<'de>>(
    http: &reqwest::Client,
    url: &str,
    token: &str,
) -> Option<T> {
    let response = http.get(url).bearer_auth(token).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<T>().await.ok()
}

/// Best-effort label for the Accounts row; failures degrade gracefully.
async fn fetch_account_label(http: &reqwest::Client, token: &str, entry: &WebhookEntry) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(user) = get_json::<UserResource>(http, &format!("{API_BASE}/users/@me"), token).await
    {
        let name = user
            .global_name
            .filter(|n| !n.is_empty())
            .unwrap_or(user.username);
        if !name.is_empty() {
            parts.push(format!("@{name}"));
        }
    }
    if !entry.guild_id.is_empty() {
        if let Some(guild) =
            get_json::<GuildResource>(http, &format!("{API_BASE}/guilds/{}", entry.guild_id), token)
                .await
        {
            if !guild.name.is_empty() {
                parts.push(guild.name);
            }
        }
    }
    if !entry.channel_id.is_empty() {
        if let Some(channel) = get_json::<ChannelResource>(
            http,
            &format!("{API_BASE}/channels/{}", entry.channel_id),
            token,
        )
        .await
        {
            if !channel.name.is_empty() {
                parts.push(format!("#{}", channel.name));
            }
        }
    }
    if parts.is_empty() {
        "Discord".to_string()
    } else {
        parts.join(" · ")
    }
}

/// Interactive connect: browser consent (pick server + channel), PKCE
/// exchange, webhook captured, OAuth grant revoked immediately.
pub async fn connect(app: &AppHandle, client_id: &str) -> Result<WebhookEntry, String> {
    let pkce = Pkce::generate();
    let loopback = Loopback::start_on(LOOPBACK_PORT, "/callback")?;
    let url = auth_url(client_id, &loopback.state, &pkce.challenge);
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("cannot open the browser: {e}"))?;
    let code = tokio::task::spawn_blocking(move || {
        loopback.wait_for_code(Duration::from_secs(180))
    })
    .await
    .map_err(|e| format!("loopback task failed: {e}"))??;

    let http = reqwest::Client::new();
    let response = http
        .post(TOKEN_URL)
        .form(&[
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("code_verifier", pkce.verifier.as_str()),
            ("redirect_uri", REDIRECT_URI),
        ])
        .send()
        .await
        .map_err(|e| format!("Discord token request failed: {e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Discord rejected the connection ({status}): {text}"));
    }
    let token: TokenResponse = response
        .json()
        .await
        .map_err(|e| format!("cannot decode the Discord response: {e}"))?;
    let resource = token
        .webhook
        .ok_or_else(|| "Discord did not return a webhook for the selected channel".to_string())?;
    let mut entry = resource.into_entry();
    entry.account = fetch_account_label(&http, &token.access_token, &entry).await;
    // The webhook is permanent; the OAuth grant is not needed anymore.
    let _ = http
        .post(REVOKE_URL)
        .form(&[
            ("client_id", client_id),
            ("token", token.access_token.as_str()),
        ])
        .send()
        .await;
    save_webhook(&entry)?;
    Ok(entry)
}

/// Remove the webhook from the Discord channel (best effort) so disconnecting
/// leaves nothing behind.
pub async fn delete_on_discord(entry: &WebhookEntry) -> Result<(), String> {
    let response = reqwest::Client::new()
        .delete(&entry.url)
        .send()
        .await
        .map_err(|e| format!("cannot delete the webhook: {e}"))?;
    if response.status().is_success() || response.status().as_u16() == 404 {
        Ok(())
    } else {
        Err(format!(
            "Discord rejected the webhook deletion ({})",
            response.status()
        ))
    }
}

/// Streaming multipart body: preamble, file chunks, epilogue. `sent` is
/// bumped as bytes leave the disk so the command can emit progress.
fn body_stream(
    file: tokio::fs::File,
    preamble: Vec<u8>,
    epilogue: Vec<u8>,
    sent: Arc<AtomicU64>,
) -> impl futures_util::Stream<Item = Result<Vec<u8>, std::io::Error>> + Send + 'static {
    use tokio::io::AsyncReadExt;
    const CHUNK: usize = 64 * 1024;

    enum Part {
        Preamble(Vec<u8>, tokio::fs::File, Vec<u8>, Vec<u8>),
        File(tokio::fs::File, Vec<u8>, Vec<u8>),
        Done,
    }

    futures_util::stream::unfold(
        Part::Preamble(preamble, file, vec![0u8; CHUNK], epilogue),
        move |part| {
            let sent = sent.clone();
            async move {
                match part {
                    Part::Preamble(preamble, file, buf, epilogue) => {
                        sent.fetch_add(preamble.len() as u64, Ordering::Relaxed);
                        Some((Ok(preamble), Part::File(file, buf, epilogue)))
                    }
                    Part::File(mut file, mut buf, epilogue) => {
                        match file.read(&mut buf).await {
                            Ok(0) => {
                                sent.fetch_add(epilogue.len() as u64, Ordering::Relaxed);
                                Some((Ok(epilogue), Part::Done))
                            }
                            Ok(n) => {
                                let bytes = buf[..n].to_vec();
                                sent.fetch_add(n as u64, Ordering::Relaxed);
                                Some((Ok(bytes), Part::File(file, buf, epilogue)))
                            }
                            Err(e) => Some((Err(e), Part::Done)),
                        }
                    }
                    Part::Done => None,
                }
            }
        },
    )
}

fn describe_error(status: reqwest::StatusCode, body: &str) -> String {
    match status.as_u16() {
        413 => "Discord rejected the file: it is larger than the server's attachment limit"
            .to_string(),
        429 => "Discord rate limited the webhook; wait a few seconds and try again".to_string(),
        401 | 403 | 404 => {
            "the Discord webhook no longer exists; reconnect Discord in Settings".to_string()
        }
        _ => format!("Discord upload failed ({status}): {body}"),
    }
}

/// POST the multipart body to the webhook with `?wait=true` (the response is
/// the posted message). Progress is observable through `sent`.
async fn upload_to_webhook(
    url: &str,
    preamble: Vec<u8>,
    epilogue: Vec<u8>,
    file: &Path,
    sent: Arc<AtomicU64>,
) -> Result<MessageResource, String> {
    let handle = tokio::fs::File::open(file)
        .await
        .map_err(|e| format!("cannot open {}: {e}", file.display()))?;
    let body = reqwest::Body::wrap_stream(body_stream(handle, preamble, epilogue, sent));
    let target = if url.contains('?') {
        format!("{url}&wait=true")
    } else {
        format!("{url}?wait=true")
    };
    let response = reqwest::Client::new()
        .post(&target)
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Discord upload failed: {e}"))?;
    let status = response.status();
    if status.is_success() {
        return response
            .json::<MessageResource>()
            .await
            .map_err(|e| format!("cannot decode the Discord response: {e}"));
    }
    let text = response.text().await.unwrap_or_default();
    Err(describe_error(status, &text))
}

/// One ffmpeg rate mode for the compressed copy (pure; unit tested).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateMode {
    Crf(u32),
    Bitrate(u32),
}

/// Compressed copy: 720p cap (never upscales), one audio track (the Mix),
/// AAC 96k, faststart. High CRF first; a computed bitrate if it still does
/// not fit.
pub fn compression_args(input: &Path, output: &Path, mode: RateMode) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "0:a:0?".into(),
        "-vf".into(),
        "scale=w=-2:h='min(720,ih)'".into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
    ];
    match mode {
        RateMode::Crf(crf) => args.extend(["-crf".into(), crf.to_string()]),
        RateMode::Bitrate(kbps) => args.extend([
            "-b:v".into(),
            format!("{kbps}k"),
            "-maxrate".into(),
            format!("{kbps}k"),
            "-bufsize".into(),
            format!("{}k", kbps * 2),
        ]),
    }
    args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "96k".into()]);
    args.extend(["-movflags".into(), "+faststart".into()]);
    args.push(output.to_string_lossy().into_owned());
    args
}

async fn run_encode(
    app: &AppHandle,
    clip_id: &str,
    ffmpeg: &Path,
    args: &[String],
    duration_ms: i64,
) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start ffmpeg: {e}"))?;
    let total = duration_ms.max(1) as f64;
    if let Some(stdout) = child.stdout.take() {
        let mut lines = BufReader::new(stdout).lines();
        let mut last = 0.0f64;
        while let Ok(Some(line)) = lines.next_line().await {
            let value = line
                .strip_prefix("out_time_us=")
                .or_else(|| line.strip_prefix("out_time_ms="));
            if let Some(v) = value {
                if let Ok(us) = v.trim().parse::<f64>() {
                    let percent = (us / 1000.0 / total * 100.0).clamp(0.0, 99.0);
                    if percent - last >= 1.0 {
                        last = percent;
                        let _ = app.emit(
                            "moonclip://publish-progress",
                            serde_json::json!({
                                "clipId": clip_id,
                                "provider": "discord",
                                "phase": "compress",
                                "sent": us / 1000.0,
                                "total": total,
                            }),
                        );
                    }
                }
            }
        }
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| format!("ffmpeg wait failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
        return Err(format!("compression failed: {tail}"));
    }
    let _ = app.emit(
        "moonclip://publish-progress",
        serde_json::json!({
            "clipId": clip_id,
            "provider": "discord",
            "phase": "compress",
            "sent": total,
            "total": total,
        }),
    );
    Ok(())
}

/// Compress until the copy fits the limit (or fail loudly).
async fn compress_for_discord(
    app: &AppHandle,
    clip_id: &str,
    input: &Path,
    output: &Path,
    limit: u64,
    ffmpeg: &Path,
) -> Result<(), String> {
    let duration_ms = crate::editor::ffmpeg::probe_duration_ms(ffmpeg, input)
        .await
        .unwrap_or(0);
    run_encode(
        app,
        clip_id,
        ffmpeg,
        &compression_args(input, output, RateMode::Crf(30)),
        duration_ms,
    )
    .await?;
    let size = tokio::fs::metadata(output).await.map(|m| m.len()).unwrap_or(u64::MAX);
    if fits(size, limit) {
        return Ok(());
    }
    if duration_ms <= 0 {
        return Err(format!(
            "the compressed copy is still too big ({} MB > {} MB); trim the clip or raise the server limit",
            size / (1024 * 1024),
            limit / (1024 * 1024)
        ));
    }
    // 10% safety margin; the audio track takes 96 kbps of the budget.
    let budget_kbps =
        (limit as f64 * 8.0 * 0.90 / 1000.0) / (duration_ms as f64 / 1000.0);
    let video_kbps = (budget_kbps - 96.0).clamp(250.0, 8000.0) as u32;
    run_encode(
        app,
        clip_id,
        ffmpeg,
        &compression_args(input, output, RateMode::Bitrate(video_kbps)),
        duration_ms,
    )
    .await?;
    let size = tokio::fs::metadata(output).await.map(|m| m.len()).unwrap_or(u64::MAX);
    if fits(size, limit) {
        Ok(())
    } else {
        Err(format!(
            "the clip does not fit Discord even compressed ({} MB > {} MB); trim it or raise the server limit",
            size / (1024 * 1024),
            limit / (1024 * 1024)
        ))
    }
}

/// Connect Discord: browser consent (server + channel) and the webhook lands
/// in the keyring. No bot, no client secret, no backend.
#[tauri::command]
pub async fn connect_discord(app: AppHandle) -> Result<super::SocialStatus, String> {
    let config = super::load_config(&app);
    let client_id = super::discord_client_id(&config)
        .ok_or_else(|| "Discord is not configured".to_string())?;
    connect(&app, &client_id).await?;
    super::social_status(app)
}

/// Disconnect Discord: remove the webhook from the channel and the keyring.
#[tauri::command]
pub async fn disconnect_discord(app: AppHandle) -> Result<super::SocialStatus, String> {
    if let Some(entry) = load_webhook()? {
        if let Err(e) = delete_on_discord(&entry).await {
            eprintln!("[moonclip] {e}");
        }
    }
    delete_webhook()?;
    super::social_status(app)
}

/// Post one clip to the connected Discord channel. Over-limit clips require
/// `compress` (a 720p copy is encoded to a temp file, uploaded and deleted).
#[tauri::command]
pub async fn discord_share_clip(
    app: AppHandle,
    clip_id: String,
    title: String,
    compress: bool,
) -> Result<DiscordShareResult, String> {
    let title = sanitize_title(&title)?;
    let entry = load_webhook()?.ok_or_else(|| "Discord is not connected".to_string())?;
    let clip = {
        let db = app.state::<DbState>();
        db.list_clips()?
            .into_iter()
            .find(|c| c.id == clip_id)
            .ok_or_else(|| "clip not found".to_string())?
    };
    let path = super::cloud::ensure_local(&app, &clip).await?;
    let size = tokio::fs::metadata(&path)
        .await
        .map_err(|e| format!("cannot stat the clip: {e}"))?
        .len();
    let limit = max_bytes(&app);
    let mut compressed: Option<PathBuf> = None;
    let upload_path = if fits(size, limit) {
        path.clone()
    } else {
        if !compress {
            return Err(format!(
                "the clip is {:.1} MB and the server limit is {} MB; enable compression or raise the limit in Settings",
                size as f64 / (1024.0 * 1024.0),
                limit / (1024 * 1024)
            ));
        }
        let ffmpeg = crate::editor::ffmpeg::resolve_ffmpeg(&app)?;
        let dir = app
            .path()
            .app_cache_dir()
            .map_err(|e| format!("no cache directory: {e}"))?
            .join("discord");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("cannot create the cache directory: {e}"))?;
        let out = dir.join(format!("{clip_id}_discord.mp4"));
        let _ = tokio::fs::remove_file(&out).await;
        compress_for_discord(&app, &clip_id, &path, &out, limit, &ffmpeg).await?;
        compressed = Some(out.clone());
        out
    };

    let payload = payload_json(&title).to_string();
    let filename = clip.file_name.rsplit('/').next().unwrap_or("clip.mp4");
    let (preamble, epilogue) = multipart_parts(&payload, filename);
    let upload_size = tokio::fs::metadata(&upload_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let total = preamble.len() as u64 + upload_size + epilogue.len() as u64;
    let sent = Arc::new(AtomicU64::new(0));
    let ticker = {
        let app = app.clone();
        let clip_id = clip_id.clone();
        let sent = sent.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(150)).await;
                let s = sent.load(Ordering::Relaxed);
                let _ = app.emit(
                    "moonclip://publish-progress",
                    serde_json::json!({
                        "clipId": clip_id,
                        "provider": "discord",
                        "phase": "upload",
                        "sent": s,
                        "total": total,
                    }),
                );
                if s >= total {
                    break;
                }
            }
        })
    };
    let posted = upload_to_webhook(&entry.url, preamble, epilogue, &upload_path, sent).await;
    ticker.abort();
    let _ = app.emit(
        "moonclip://publish-progress",
        serde_json::json!({
            "clipId": clip_id,
            "provider": "discord",
            "phase": "upload",
            "sent": total,
            "total": total,
        }),
    );
    if let Some(file) = compressed {
        let _ = tokio::fs::remove_file(file).await;
    }
    if clip.cloud {
        // The local copy was only the on-demand cache; the clip stays cloud.
        super::cloud::cleanup_cache(&app, &clip.id);
    }
    let posted = posted?;
    let url = if entry.guild_id.is_empty() {
        None
    } else {
        Some(format!(
            "https://discord.com/channels/{}/{}/{}",
            entry.guild_id, posted.channel_id, posted.id
        ))
    };
    Ok(DiscordShareResult {
        message_id: posted.id,
        url,
        channel: entry.account,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_url_carries_pkce_state_and_scopes() {
        let url = auth_url("1553938269874557059", "state123", "challenge123");
        assert!(url.starts_with(AUTH_URL));
        assert!(url.contains("client_id=1553938269874557059"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("scope=webhook.incoming%20identify%20guilds"));
        assert!(url.contains("state=state123"));
        assert!(url.contains("code_challenge=challenge123"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A38471%2Fcallback"
        ));
    }

    #[test]
    fn title_is_sanitized_and_capped() {
        assert_eq!(sanitize_title("  My   clip  ").unwrap(), "My clip");
        let long = "a".repeat(150);
        assert_eq!(sanitize_title(&long).unwrap().chars().count(), 100);
        assert!(sanitize_title("   ").is_err());
    }

    #[test]
    fn payload_carries_tags_and_disables_mentions() {
        let payload = payload_json("Ace");
        assert_eq!(payload["content"], "Ace #MoonClip #moonclip");
        assert_eq!(payload["allowed_mentions"]["parse"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn webhook_token_response_parses() {
        let text = r#"{
            "token_type": "Bearer",
            "access_token": "at",
            "scope": "webhook.incoming",
            "webhook": {
                "id": "347114750880120863",
                "token": "wh-token",
                "url": "https://discord.com/api/webhooks/347114750880120863/wh-token",
                "channel_id": "345626669224982402",
                "guild_id": "290926792226357250",
                "name": "MoonClip"
            }
        }"#;
        let token: TokenResponse = serde_json::from_str(text).unwrap();
        assert_eq!(token.access_token, "at");
        let entry = token.webhook.unwrap().into_entry();
        assert_eq!(entry.id, "347114750880120863");
        assert_eq!(entry.token, "wh-token");
        assert!(entry.url.ends_with("/347114750880120863/wh-token"));
        assert_eq!(entry.guild_id, "290926792226357250");
        assert_eq!(entry.channel_id, "345626669224982402");
    }

    #[test]
    fn webhook_without_url_is_rebuilt() {
        let resource = WebhookResource {
            id: "1".into(),
            token: "t".into(),
            url: String::new(),
            guild_id: String::new(),
            channel_id: String::new(),
        };
        let entry = resource.into_entry();
        assert_eq!(entry.url, "https://discord.com/api/v10/webhooks/1/t");
    }

    #[test]
    fn multipart_parts_frame_payload_and_file() {
        let payload = r#"{"content":"Ace #MoonClip #moonclip"}"#;
        let (preamble, epilogue) = multipart_parts(payload, "clip \"1\".mp4");
        let text = String::from_utf8(preamble).unwrap();
        assert!(text.starts_with(&format!("--{BOUNDARY}\r\n")));
        assert!(text.contains("name=\"payload_json\""));
        assert!(text.contains(payload));
        assert!(text.contains("name=\"files[0]\""));
        assert!(text.contains("filename=\"clip _1_.mp4\""));
        assert_eq!(epilogue, format!("\r\n--{BOUNDARY}--\r\n").into_bytes());
    }

    #[test]
    fn size_gate_is_inclusive() {
        assert!(fits(10, 10));
        assert!(fits(9, 10));
        assert!(!fits(11, 10));
    }

    #[test]
    fn compression_args_cap_height_and_keep_one_audio_track() {
        let args = compression_args(
            Path::new("/in/clip.mp4"),
            Path::new("/out/clip.mp4"),
            RateMode::Crf(30),
        );
        let joined = args.join(" ");
        assert!(joined.contains("-map 0:v:0 -map 0:a:0?"));
        assert!(joined.contains("scale=w=-2:h='min(720,ih)'"));
        assert!(joined.contains("-c:v libx264 -preset veryfast -pix_fmt yuv420p -crf 30"));
        assert!(joined.contains("-c:a aac -b:a 96k"));
        assert!(joined.contains("-movflags +faststart"));
        assert_eq!(args.last().unwrap(), "/out/clip.mp4");

        let bitrate = compression_args(
            Path::new("/in/clip.mp4"),
            Path::new("/out/clip.mp4"),
            RateMode::Bitrate(1200),
        )
        .join(" ");
        assert!(bitrate.contains("-b:v 1200k -maxrate 1200k -bufsize 2400k"));
        assert!(!bitrate.contains("-crf"));
    }

    /// Live-ish: the multipart upload against a local mock. Asserts the body
    /// carries the payload and the exact file bytes, and that the progress
    /// counter reaches the full length.
    #[tokio::test]
    async fn multipart_upload_posts_the_file_and_reports_progress() {
        let dir = std::env::temp_dir().join(format!(
            "moonclip-discord-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.mp4");
        let payload: Vec<u8> = (0..(200 * 1024u32)).map(|i| (i % 241) as u8).collect();
        std::fs::write(&file, &payload).unwrap();

        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let body = r#"{"id":"msg1","channel_id":"chan1"}"#;
        let expected = payload.clone();
        let handle = std::thread::spawn(move || {
            let mut request = server.recv().unwrap();
            assert_eq!(request.method(), &tiny_http::Method::Post);
            assert_eq!(request.url(), "/webhook?wait=true");
            let mut received = Vec::new();
            request.as_reader().read_to_end(&mut received).unwrap();
            let text = String::from_utf8_lossy(&received).to_string();
            assert!(text.contains("\"content\":\"Mock clip #MoonClip #moonclip\""));
            assert!(text.contains("filename=\"clip.mp4\""));
            let marker = b"Content-Type: video/mp4\r\n\r\n";
            let body_start = received
                .windows(marker.len())
                .position(|w| w == marker)
                .map(|i| i + marker.len())
                .unwrap();
            let epilogue = format!("\r\n--{BOUNDARY}--\r\n");
            let body_end = received.len() - epilogue.len();
            assert_eq!(&received[body_start..body_end], expected.as_slice());
            request
                .respond(tiny_http::Response::from_string(body))
                .unwrap();
        });

        let (preamble, epilogue) = multipart_parts(
            &payload_json("Mock clip").to_string(),
            "clip.mp4",
        );
        let total = preamble.len() as u64 + payload.len() as u64 + epilogue.len() as u64;
        let sent = Arc::new(AtomicU64::new(0));
        let posted = upload_to_webhook(
            &format!("http://127.0.0.1:{port}/webhook"),
            preamble,
            epilogue,
            &file,
            sent.clone(),
        )
        .await
        .unwrap();
        assert_eq!(posted.id, "msg1");
        assert_eq!(posted.channel_id, "chan1");
        assert_eq!(sent.load(Ordering::Relaxed), total);
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 413 is mapped to the actionable limit message.
    #[tokio::test]
    async fn over_limit_uploads_get_a_clear_error() {
        let dir = std::env::temp_dir().join(format!(
            "moonclip-discord-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.mp4");
        std::fs::write(&file, b"tiny").unwrap();
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let handle = std::thread::spawn(move || {
            let mut request = server.recv().unwrap();
            let mut received = Vec::new();
            request.as_reader().read_to_end(&mut received).unwrap();
            request
                .respond(tiny_http::Response::from_string("{\"message\":\"too big\"}").with_status_code(413))
                .unwrap();
        });
        let (preamble, epilogue) = multipart_parts("{}", "clip.mp4");
        let sent = Arc::new(AtomicU64::new(0));
        let err = upload_to_webhook(
            &format!("http://127.0.0.1:{port}/webhook"),
            preamble,
            epilogue,
            &file,
            sent,
        )
        .await
        .unwrap_err();
        assert!(err.contains("attachment limit"), "{err}");
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
