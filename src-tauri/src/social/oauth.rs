//! OAuth 2.0 plumbing shared by every provider: PKCE (S256), a single-use
//! loopback redirect server and the token endpoint calls. RFC 8252/7636.

use std::time::{Duration, Instant};

use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// PKCE verifier/challenge pair. The verifier is 64 hex chars (valid
/// unreserved set, 43–128 length); the challenge is base64url(SHA256).
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Self {
        let verifier = format!(
            "{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        Self {
            verifier,
            challenge,
        }
    }
}

/// Single-use loopback redirect listener on an ephemeral port.
pub struct Loopback {
    server: tiny_http::Server,
    pub redirect_uri: String,
    pub state: String,
    path: String,
}

impl Loopback {
    pub fn start(path: &str) -> Result<Self, String> {
        let server = tiny_http::Server::http("127.0.0.1:0")
            .map_err(|e| format!("cannot start the loopback server: {e}"))?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|addr| addr.port())
            .ok_or_else(|| "loopback server has no TCP address".to_string())?;
        let state = format!(
            "{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        Ok(Self {
            server,
            redirect_uri: format!("http://127.0.0.1:{port}{path}"),
            state,
            path: path.to_string(),
        })
    }

    /// Blocking: serve requests until the OAuth redirect arrives, then answer
    /// the browser and return the code. Wrong path/state is ignored (a stray
    /// request must not cancel the flow).
    pub fn wait_for_code(self, timeout: Duration) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self
                .server
                .recv_timeout(Duration::from_millis(500))
            {
                Ok(Some(request)) => {
                    let (matched, code) =
                        parse_callback(request.url(), &self.path, &self.state);
                    let (status, body) = if matched {
                        (200, DONE_HTML)
                    } else {
                        (404, ERROR_HTML)
                    };
                    let response = tiny_http::Response::from_string(body)
                        .with_status_code(status)
                        .with_header(
                            tiny_http::Header::from_bytes(
                                &b"Content-Type"[..],
                                &b"text/html; charset=utf-8"[..],
                            )
                            .expect("valid header"),
                        );
                    let _ = request.respond(response);
                    if matched {
                        return code.ok_or_else(|| "missing authorization code".to_string());
                    }
                }
                Ok(None) => continue,
                Err(e) => return Err(format!("loopback server error: {e}")),
            }
        }
        Err("timed out waiting for the browser authorization".into())
    }
}

/// Parse `/callback?code=...&state=...`. Pure so it can be unit tested.
fn parse_callback(url: &str, path: &str, expected_state: &str) -> (bool, Option<String>) {
    let Some((got_path, query)) = url.split_once('?') else {
        return (false, None);
    };
    if got_path != path {
        return (false, None);
    }
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (key, raw) = pair.split_once('=').unwrap_or((pair, ""));
        let value = urlencoding::decode(raw)
            .map(|v| v.into_owned())
            .unwrap_or_default();
        match key {
            "code" => code = Some(value),
            "state" => state = Some(value),
            "error" => error = Some(value),
            _ => {}
        }
    }
    if error.is_some() {
        return (false, None);
    }
    match (code, state) {
        (Some(code), Some(state)) if state == expected_state => (true, Some(code)),
        _ => (false, None),
    }
}

/// Successful token endpoint response (Google/TikTok shapes are compatible).
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

pub async fn exchange_code(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, String> {
    let form = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];
    post_token(token_url, &form).await
}

pub async fn refresh(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<TokenResponse, String> {
    let form = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    post_token(token_url, &form).await
}

async fn post_token(token_url: &str, form: &[(&str, &str)]) -> Result<TokenResponse, String> {
    let response = reqwest::Client::new()
        .post(token_url)
        .form(form)
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("token endpoint rejected the request ({status}): {text}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("cannot decode the token response: {e}"))
}

const DONE_HTML: &str = "<!doctype html><html><body style=\"background:#0d0f14;color:#e6e9f0;font-family:system-ui;display:grid;place-items:center;height:100vh\"><div style=\"text-align:center\"><h2>MoonClip</h2><p>Listo. Puedes cerrar esta pestaña y volver a la app.</p></div></body></html>";
const ERROR_HTML: &str = "<!doctype html><html><body style=\"background:#0d0f14;color:#e6e9f0;font-family:system-ui;display:grid;place-items:center;height:100vh\"><div style=\"text-align:center\"><h2>MoonClip</h2><p>No se pudo completar la conexión. Vuelve a intentarlo desde la app.</p></div></body></html>";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_vector() {
        // RFC 7636 appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn generated_pkce_is_well_formed() {
        let pkce = Pkce::generate();
        assert!((43..=128).contains(&pkce.verifier.len()));
        assert!(!pkce.challenge.contains('='));
        assert!(!pkce.challenge.contains('+'));
        assert!(!pkce.challenge.contains('/'));
        // A fresh verifier every time.
        assert_ne!(pkce.verifier, Pkce::generate().verifier);
    }

    #[test]
    fn callback_requires_the_exact_state_and_path() {
        let ok = parse_callback("/callback?code=abc&state=st", "/callback", "st");
        assert_eq!(ok, (true, Some("abc".to_string())));
        assert!(!parse_callback("/callback?code=abc&state=other", "/callback", "st").0);
        assert!(!parse_callback("/other?code=abc&state=st", "/callback", "st").0);
        assert!(!parse_callback("/callback", "/callback", "st").0);
        assert!(!parse_callback(
            "/callback?error=access_denied&state=st",
            "/callback",
            "st"
        )
        .0);
    }

    #[test]
    fn callback_decodes_url_encoded_codes() {
        let (ok, code) = parse_callback(
            "/callback?code=4%2F0Aabc%2Bd&state=st",
            "/callback",
            "st",
        );
        assert!(ok);
        assert_eq!(code.unwrap(), "4/0Aabc+d");
    }

    #[test]
    fn loopback_binds_and_reports_its_redirect_uri() {
        let loopback = Loopback::start("/callback").unwrap();
        assert!(loopback.redirect_uri.starts_with("http://127.0.0.1:"));
        assert!(loopback.redirect_uri.ends_with("/callback"));
        assert!(!loopback.state.is_empty());
    }

    /// Live-ish: a local token endpoint mock returns a valid response.
    #[tokio::test]
    async fn exchange_and_refresh_parse_the_mock_endpoint() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/token", server.server_addr());
        let handle = std::thread::spawn(move || {
            for _ in 0..2 {
                let request = server.recv().unwrap();
                let body = r#"{"access_token":"at","expires_in":3600,"refresh_token":"rt","scope":"s"}"#;
                let response = tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json"[..],
                    )
                    .unwrap(),
                );
                request.respond(response).unwrap();
            }
        });
        let exchanged = exchange_code(&url, "id", "sec", "code", "verifier", "http://127.0.0.1/cb")
            .await
            .unwrap();
        assert_eq!(exchanged.access_token, "at");
        assert_eq!(exchanged.refresh_token.as_deref(), Some("rt"));
        let refreshed = refresh(&url, "id", "sec", "rt").await.unwrap();
        assert_eq!(refreshed.access_token, "at");
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn token_errors_are_reported_with_the_body() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/token", server.server_addr());
        let handle = std::thread::spawn(move || {
            let request = server.recv().unwrap();
            let response = tiny_http::Response::from_string(r#"{"error":"invalid_grant"}"#)
                .with_status_code(400);
            request.respond(response).unwrap();
        });
        let err = exchange_code(&url, "id", "sec", "bad", "v", "http://127.0.0.1/cb")
            .await
            .unwrap_err();
        assert!(err.contains("invalid_grant"), "{err}");
        handle.join().unwrap();
    }
}
