use keyring::Entry;
use serde::{Deserialize, Serialize};

use crate::gmail::models::TokenResponse;
use crate::gmail::GmailError;

const SERVICE_NAME: &str = "post-office";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenStore {
    pub access: String,
    pub refresh: String,
}

pub struct GmailAuth {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) client_id: String,
    pub(crate) client_secret: Option<String>,
    pub(crate) expires_at: std::time::Instant,
}

impl GmailAuth {
    pub fn new(
        access_token: String,
        refresh_token: String,
        client_id: String,
        client_secret: Option<String>,
        expires_in: u32,
    ) -> Self {
        Self {
            access_token,
            refresh_token,
            client_id,
            client_secret,
            expires_at: std::time::Instant::now()
                + std::time::Duration::from_secs(expires_in.into()),
        }
    }

    pub fn is_expired(&self) -> bool {
        // Refresh 5 minutes before expiry
        self.expires_at <= std::time::Instant::now() + std::time::Duration::from_secs(300)
    }

    pub async fn refresh(&mut self, http: &reqwest::Client) -> Result<(), GmailError> {
        let mut form = vec![
            ("client_id", self.client_id.as_str()),
            ("refresh_token", self.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];
        if let Some(secret) = self.client_secret.as_deref() {
            form.push(("client_secret", secret));
        }

        let token: TokenResponse = http
            .post("https://oauth2.googleapis.com/token")
            .form(&form)
            .send()
            .await?
            .json()
            .await?;

        self.access_token = token.access_token;
        self.expires_at =
            std::time::Instant::now() + std::time::Duration::from_secs(token.expires_in.into());

        Ok(())
    }

    pub fn load(account: &str, client_id: &str, client_secret: Option<&str>) -> Option<Self> {
        let tokens = load_tokens(account).ok()??;
        if client_id.is_empty() {
            return None;
        }
        Some(Self::new(
            tokens.access,
            tokens.refresh,
            client_id.to_string(),
            client_secret.map(|s| s.to_string()),
            3600,
        ))
    }
}

pub fn store_tokens(account: &str, access: &str, refresh: &str) -> Result<(), GmailError> {
    let entry = Entry::new(SERVICE_NAME, account)?;
    let tokens = TokenStore {
        access: access.to_string(),
        refresh: refresh.to_string(),
    };
    let _ = entry.set_password(&serde_json::to_string(&tokens)?)?;
    Ok(())
}

pub fn load_tokens(account: &str) -> Result<Option<TokenStore>, GmailError> {
    let entry = Entry::new(SERVICE_NAME, account)?;
    match entry.get_password() {
        Ok(json) => serde_json::from_str(&json).map(Some).map_err(Into::into),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
