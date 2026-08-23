pub mod client;
pub mod credentials;
pub mod router;

pub use client::LlmClient;
pub use router::{InferenceRouter, LlmProviderProfile, LlmRoutingPolicy, PrivacyRequirement};

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("LLM API error: {0}")]
    Api(#[from] async_openai::error::OpenAIError),

    #[error("No response from LLM")]
    NoResponse,

    #[error("Failed to parse LLM JSON response: {0}")]
    ParseError(String),

    #[error("No eligible LLM provider was available: {0}")]
    Routing(String),
}

impl LlmError {
    pub fn user_message(&self) -> String {
        let Self::Api(async_openai::error::OpenAIError::JSONDeserialize(_, content)) = self else {
            return self.to_string();
        };
        let Some(error) = serde_json::from_str::<serde_json::Value>(content)
            .ok()
            .and_then(|response| response.get("error").cloned())
        else {
            return self.to_string();
        };
        let Some(message) = error.get("message").and_then(serde_json::Value::as_str) else {
            return self.to_string();
        };
        let code = error.get("code").and_then(|code| {
            code.as_str()
                .map(str::to_owned)
                .or_else(|| code.as_i64().map(|code| code.to_string()))
        });
        match code {
            Some(code) => format!("LLM API error: {code}: {message}"),
            None => format!("LLM API error: {message}"),
        }
    }
}

#[derive(Clone, Default)]
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

pub struct ProcessResponse {
    pub content: String,
    pub model: String,
    pub provider_id: Option<String>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub tokens_used: Option<u32>,
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_numeric_provider_error_codes() {
        let content = r#"{"error":{"message":"No eligible endpoints","code":404}}"#;
        let decode_error = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let error = LlmError::Api(async_openai::error::OpenAIError::JSONDeserialize(
            decode_error,
            content.into(),
        ));

        assert_eq!(
            error.user_message(),
            "LLM API error: 404: No eligible endpoints"
        );
    }
}
