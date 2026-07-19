use thiserror::Error;

#[derive(Debug, Error)]
pub enum GmailError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Keyring(#[from] keyring::Error),

    #[error("API error: {code} - {message}")]
    Api { code: u16, message: String },

    #[error("token refresh failed")]
    TokenRefresh,

    #[error("{0}")]
    Auth(String),
}
