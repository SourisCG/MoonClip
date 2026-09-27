//! OAuth token blobs in the OS keyring. One JSON entry per provider; the
//! access token is refreshed on demand and the blob rewritten (TikTok rotates
//! refresh tokens, Google does not).

use serde::{Deserialize, Serialize};

use crate::storage::secrets;

pub const GOOGLE_DRIVE: &str = "oauth_google_drive";
pub const GOOGLE_YOUTUBE: &str = "oauth_google_youtube";
pub const TIKTOK: &str = "oauth_tiktok";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// Unix seconds; 0 = unknown (treated as expired).
    #[serde(default)]
    pub expires_at: i64,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub account: String,
}

impl OAuthTokens {
    pub fn new(
        access_token: String,
        refresh_token: String,
        expires_in: Option<i64>,
        scope: String,
        account: String,
    ) -> Self {
        Self {
            access_token,
            refresh_token,
            expires_at: now_unix() + expires_in.unwrap_or(0).max(0),
            scope,
            account,
        }
    }

    /// Refresh a bit before the real expiry to avoid edge races.
    pub fn needs_refresh(&self) -> bool {
        self.expires_at <= now_unix() + 120
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn save(alias: &str, tokens: &OAuthTokens) -> Result<(), String> {
    let text = serde_json::to_string(tokens).map_err(|e| format!("cannot encode tokens: {e}"))?;
    secrets::store_secret(alias, &text)
}

pub fn load(alias: &str) -> Result<Option<OAuthTokens>, String> {
    match secrets::get_secret_opt(alias)? {
        Some(text) => match serde_json::from_str::<OAuthTokens>(&text) {
            Ok(tokens) => Ok(Some(tokens)),
            Err(e) => {
                eprintln!("[moonclip] stored token blob for {alias} is unreadable: {e}");
                Ok(None)
            }
        },
        None => Ok(None),
    }
}

pub fn delete(alias: &str) -> Result<(), String> {
    secrets::delete_secret(alias)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_logic_uses_a_safety_margin() {
        let mut tokens = OAuthTokens::new(
            "at".into(),
            "rt".into(),
            Some(3600),
            "s".into(),
            "me@example.com".into(),
        );
        assert!(!tokens.needs_refresh());
        tokens.expires_at = now_unix() + 60;
        assert!(tokens.needs_refresh());
        tokens.expires_at = now_unix() - 1;
        assert!(tokens.needs_refresh());
    }

    #[test]
    fn unknown_expiry_is_treated_as_expired() {
        let tokens = OAuthTokens::new("at".into(), "rt".into(), None, "s".into(), String::new());
        assert!(tokens.needs_refresh());
    }

    #[test]
    fn blob_round_trips_through_json() {
        let tokens = OAuthTokens::new(
            "at".into(),
            "rt".into(),
            Some(100),
            "scope".into(),
            "acc".into(),
        );
        let text = serde_json::to_string(&tokens).unwrap();
        let back: OAuthTokens = serde_json::from_str(&text).unwrap();
        assert_eq!(back.access_token, "at");
        assert_eq!(back.refresh_token, "rt");
        assert_eq!(back.account, "acc");
    }
}
