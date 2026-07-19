use base64::Engine;
use rand::RngCore;
use reqwest::Client;
use sha2::{Digest, Sha256};

use crate::gmail::models::TokenResponse;
use crate::GmailError;

pub const REDIRECT_URI: &str = "http://localhost:7890";

include!(concat!(env!("OUT_DIR"), "/oauth_client_id.rs"));

/// The Google OAuth client_id baked into the binary at compile time.
pub fn baked_client_id() -> &'static str {
    BAKED_GOOGLE_CLIENT_ID
}

/// The Google OAuth client_secret baked into the binary, if provided.
pub fn baked_client_secret() -> &'static str {
    BAKED_GOOGLE_CLIENT_SECRET
}

#[derive(Debug, Clone)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u32,
}

/// Generate a PKCE code_verifier and its S256 code_challenge.
pub fn generate_pkce() -> (String, String) {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());

    (verifier, challenge)
}

pub fn auth_url(client_id: &str, challenge: &str, state: &str) -> String {
    format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope=https://www.googleapis.com/auth/gmail.modify+https://www.googleapis.com/auth/gmail.labels&access_type=offline&prompt=consent&state={}&code_challenge={}&code_challenge_method=S256",
        client_id, REDIRECT_URI, state, challenge
    )
}

/// Exchange the authorization code for tokens using PKCE. The client_secret
/// is included when available (Google Desktop clients expect it); PKCE's
/// code_verifier is always sent.
pub async fn exchange_code(
    http: &Client,
    client_id: &str,
    code: &str,
    code_verifier: &str,
    client_secret: Option<&str>,
) -> Result<OAuthTokens, GmailError> {
    let mut form = vec![
        ("client_id", client_id),
        ("code", code),
        ("code_verifier", code_verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", REDIRECT_URI),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }

    let response = http
        .post("https://oauth2.googleapis.com/token")
        .form(&form)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(GmailError::Auth(format!(
            "token endpoint returned {}: {}",
            status, body
        )));
    }

    let token: TokenResponse = response
        .json()
        .await
        .map_err(|e| GmailError::Auth(format!("failed to decode token response: {}", e)))?;

    Ok(OAuthTokens {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_in: token.expires_in,
    })
}
