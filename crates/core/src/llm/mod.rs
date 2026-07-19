pub mod client;

pub use client::LlmClient;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("LLM API error: {0}")]
    Api(#[from] async_openai::error::OpenAIError),

    #[error("No response from LLM")]
    NoResponse,

    #[error("Failed to parse LLM JSON response: {0}")]
    ParseError(String),
}

pub struct ProcessRequest {
    pub system_prompt: Option<String>,
    pub user_prompt: String,
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl ProcessRequest {
    pub fn new(user_prompt: impl Into<String>) -> Self {
        Self {
            system_prompt: None,
            user_prompt: user_prompt.into(),
            model: None,
            temperature: None,
            max_tokens: None,
        }
    }
}

impl Default for ProcessRequest {
    fn default() -> Self {
        Self {
            system_prompt: None,
            user_prompt: String::new(),
            model: None,
            temperature: None,
            max_tokens: None,
        }
    }
}

pub struct ProcessResponse {
    pub content: String,
    pub model: String,
    pub tokens_used: Option<u32>,
    pub duration_ms: u64,
}
