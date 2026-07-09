use async_openai::{
    config::OpenAIConfig,
    Client,
};
use serde_json::{json, Value};

use super::{LlmError, ProcessRequest, ProcessResponse};

pub struct LlmClient {
    client: Client<OpenAIConfig>,
    default_model: String,
}

impl LlmClient {
    pub fn new(base_url: &str, api_key: &str, default_model: &str) -> Self {
        let config = OpenAIConfig::new()
            .with_api_base(base_url)
            .with_api_key(api_key);

        Self {
            client: Client::with_config(config),
            default_model: default_model.to_string(),
        }
    }

    pub async fn process(&self, request: ProcessRequest) -> Result<ProcessResponse, LlmError> {
        let model = request
            .model
            .unwrap_or_else(|| self.default_model.clone());

        let mut messages = Vec::new();

        if let Some(system_prompt) = &request.system_prompt {
            messages.push(json!({
                "role": "system",
                "content": system_prompt
            }));
        }

        messages.push(json!({
            "role": "user",
            "content": request.user_prompt
        }));

        let start = std::time::Instant::now();

        let response: Value = self
            .client
            .chat()
            .create_byot(json!({
                "model": model,
                "messages": messages,
                "temperature": request.temperature,
                "max_tokens": request.max_tokens,
            }))
            .await?;

        let duration_ms = start.elapsed().as_millis() as u64;

        let content = response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let tokens_used = response["usage"]["total_tokens"]
            .as_u64()
            .map(|t| t as u32);

        let model_used = response["model"]
            .as_str()
            .unwrap_or(&model)
            .to_string();

        Ok(ProcessResponse {
            content,
            model: model_used,
            tokens_used,
            duration_ms,
        })
    }
}
