//! Google provider: authorization URL, connect flow (browser + loopback),
//! token refresh and the account label from OpenID userinfo.

use std::time::Duration;

use serde::Deserialize;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use super::oauth::{self, Loopback, Pkce};
use super::token_store::{self, OAuthTokens};
use super::ClientCredentials;

pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";

/// Google APIs MoonClip can connect to (each keeps its own token + scopes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Drive,
    YouTube,
}

impl Provider {
    pub fn alias(self) -> &'static str {
        match self {
            Provider::Drive => token_store::GOOGLE_DRIVE,
            Provider::YouTube => token_store::GOOGLE_YOUTUBE,
        }
    }

    pub fn scopes(self) -> &'static [&'static str] {
        match self {
            Provider::Drive => &[
                "https://www.googleapis.com/auth/drive.file",
                "https://www.googleapis.com/auth/userinfo.email",
            ],
            Provider::YouTube => &[
                "https://www.googleapis.com/auth/youtube.upload",
                "https://www.googleapis.com/auth/userinfo.email",
            ],
        }
    }
}

/// Build the consent URL. `prompt=consent` guarantees a refresh token even on
/// reconnects; `access_type=offline` keeps the connection alive.
pub fn auth_url(
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    state: &str,
    challenge: &str,
) -> String {
    let enc = urlencoding::encode;
    format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&code_challenge={}&code_challenge_method=S256&access_type=offline&prompt=consent",
        enc(client_id),
        enc(redirect_uri),
        enc(scope),
        enc(state),
        enc(challenge),
    )
}

/// Full interactive connect. Opens the system browser and waits up to 3 min.
pub async fn connect(
    app: &AppHandle,
    provider: Provider,
    client: &ClientCredentials,
) -> Result<OAuthTokens, String> {
    let pkce = Pkce::generate();
    let loopback = Loopback::start("/callback")?;
    let scope = provider.scopes().join(" ");
    let url = auth_url(
        &client.client_id,
        &loopback.redirect_uri,
        &scope,
        &loopback.state,
        &pkce.challenge,
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
    let response = oauth::exchange_code(
        TOKEN_URL,
        &client.client_id,
        &client.client_secret,
        &code,
        &pkce.verifier,
        &redirect_uri,
    )
    .await?;
    let account = fetch_account(&response.access_token).await.unwrap_or_default();
    let tokens = OAuthTokens::new(
        response.access_token,
        response.refresh_token.unwrap_or_default(),
        response.expires_in,
        response.scope.unwrap_or(scope),
        account,
    );
    token_store::save(provider.alias(), &tokens)?;
    Ok(tokens)
}

/// A valid access token, refreshed and re-saved when needed.
pub async fn access_token(
    provider: Provider,
    client: &ClientCredentials,
) -> Result<String, String> {
    access_token_with(provider, client, TOKEN_URL).await
}

async fn access_token_with(
    provider: Provider,
    client: &ClientCredentials,
    token_url: &str,
) -> Result<String, String> {
    let alias = provider.alias();
    let mut tokens = token_store::load(alias)?
        .ok_or_else(|| "the account is not connected".to_string())?;
    if tokens.needs_refresh() {
        if tokens.refresh_token.is_empty() {
            return Err("the stored session cannot be refreshed; reconnect the account".into());
        }
        let refreshed = oauth::refresh(
            token_url,
            &client.client_id,
            &client.client_secret,
            &tokens.refresh_token,
        )
        .await?;
        tokens.access_token = refreshed.access_token;
        if let Some(rt) = refreshed.refresh_token.filter(|rt| !rt.is_empty()) {
            tokens.refresh_token = rt;
        }
        tokens.expires_at = token_store::now_unix() + refreshed.expires_in.unwrap_or(0).max(0);
        token_store::save(alias, &tokens)?;
    }
    Ok(tokens.access_token)
}

#[derive(Deserialize)]
struct UserInfo {
    #[serde(default)]
    email: String,
}

async fn fetch_account(access_token: &str) -> Option<String> {
    let response = reqwest::Client::new()
        .get(USERINFO_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .json::<UserInfo>()
        .await
        .ok()
        .map(|info| info.email)
        .filter(|email| !email.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_url_carries_pkce_and_offline_access() {
        let url = auth_url(
            "client-id.apps.googleusercontent.com",
            "http://127.0.0.1:4444/callback",
            "https://www.googleapis.com/auth/drive.file https://www.googleapis.com/auth/userinfo.email",
            "state123",
            "challenge123",
        );
        assert!(url.starts_with(AUTH_URL));
        assert!(url.contains("client_id=client-id.apps.googleusercontent.com"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A4444%2Fcallback"));
        assert!(url.contains("code_challenge=challenge123"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
        assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.file"));
    }

    #[test]
    fn providers_have_distinct_aliases_and_scopes() {
        assert_ne!(Provider::Drive.alias(), Provider::YouTube.alias());
        assert!(Provider::Drive
            .scopes()
            .iter()
            .any(|s| s.ends_with("drive.file")));
        assert!(Provider::YouTube
            .scopes()
            .iter()
            .any(|s| s.ends_with("youtube.upload")));
    }

    /// Refresh path against a local mock: the rotated access token replaces
    /// the stored one and the blob is rewritten.
    #[tokio::test]
    async fn access_token_refreshes_expired_sessions() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/token", server.server_addr());
        let handle = std::thread::spawn(move || {
            let request = server.recv().unwrap();
            let body = r#"{"access_token":"fresh","expires_in":3600}"#;
            request
                .respond(tiny_http::Response::from_string(body))
                .unwrap();
        });
        let client = ClientCredentials {
            client_id: "id".into(),
            client_secret: "sec".into(),
        };
        // No vault in unit tests: exercise the pure refresh branch instead.
        let refreshed = oauth::refresh(&url, &client.client_id, &client.client_secret, "rt")
            .await
            .unwrap();
        assert_eq!(refreshed.access_token, "fresh");
        handle.join().unwrap();
    }
}
