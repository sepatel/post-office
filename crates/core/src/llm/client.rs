use async_openai::{config::OpenAIConfig, Client};
use serde_json::{json, Value};
use std::time::Duration;

use super::{LlmError, ProcessRequest, ProcessResponse};

#[derive(Clone)]
pub struct LlmClient {
    client: Client<OpenAIConfig>,
    default_model: String,
    provider_id: Option<String>,
}

impl LlmClient {
    pub fn new(base_url: &str, api_key: &str, default_model: &str) -> Self {
        Self::with_options(base_url, api_key, default_model, 30, None)
    }

    pub fn with_options(
        base_url: &str,
        api_key: &str,
        default_model: &str,
        timeout_secs: u64,
        provider_id: Option<String>,
    ) -> Self {
        let config = OpenAIConfig::new()
            .with_api_base(base_url)
            .with_api_key(api_key);
        let http = reqwest13::Client::builder()
            .timeout(Duration::from_secs(timeout_secs.max(1)))
            .build()
            .unwrap_or_else(|_| reqwest13::Client::new());

        Self {
            client: Client::with_config(config).with_http_client(http),
            default_model: default_model.to_string(),
            provider_id,
        }
    }

    pub async fn process(&self, request: ProcessRequest) -> Result<ProcessResponse, LlmError> {
        let model = request.model.unwrap_or_else(|| self.default_model.clone());

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

        let mut body = json!({
            "model": model,
            "messages": messages,
        });
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }

        let response: Value = self.client.chat().create_byot(body).await?;

        let duration_ms = start.elapsed().as_millis() as u64;

        let content = response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let prompt_tokens = response["usage"]["prompt_tokens"]
            .as_u64()
            .map(|t| t as u32);
        let completion_tokens = response["usage"]["completion_tokens"]
            .as_u64()
            .map(|t| t as u32);
        let tokens_used = response["usage"]["total_tokens"].as_u64().map(|t| t as u32);

        let model_used = response["model"].as_str().unwrap_or(&model).to_string();

        Ok(ProcessResponse {
            content,
            model: model_used,
            provider_id: self.provider_id.clone(),
            prompt_tokens,
            completion_tokens,
            tokens_used,
            duration_ms,
        })
    }

    /// Lists model ids from the OpenAI-compatible `/models` endpoint. Lets the
    /// settings UI offer a dropdown of valid models instead of free-typing the
    /// id (and guessing wrong like a bad base URL).
    pub async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let resp = self.client.models().list().await?;
        Ok(resp.data.into_iter().map(|m| m.id).collect())
    }

    /// Reachability/validity check: sends a tiny completion and returns the
    /// model the endpoint actually served (which can differ from the requested
    /// id when the server aliases it). Surfaces endpoint/auth/model problems
    /// early instead of failing silently during a rule test or apply.
    pub async fn test_connection(&self) -> Result<ProcessResponse, LlmError> {
        self.process(ProcessRequest {
            system_prompt: Some("Reply with the single word OK.".into()),
            user_prompt: "Test.".into(),
            model: None,
            temperature: Some(0.0),
            max_tokens: Some(5),
        })
        .await
    }

    /// Chat that expects a JSON object back. Used by the rule-tuning chat, where
    /// the model returns a structured proposal. Strips optional markdown code
    /// fences before parsing, since local models don't always honor
    /// `response_format`.
    pub async fn chat_json(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<serde_json::Value, LlmError> {
        let model = self.default_model.clone();
        let messages = vec![
            json!({ "role": "system", "content": system_prompt }),
            json!({ "role": "user", "content": user_prompt }),
        ];

        let response: Value = self
            .client
            .chat()
            .create_byot(json!({
                "model": model,
                "messages": messages,
                "temperature": 0.3f32,
            }))
            .await?;

        let content = response["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default();

        let content = strip_code_fence(content);
        serde_json::from_str(&content).map_err(|e| LlmError::ParseError(e.to_string()))
    }
}

fn strip_code_fence(content: &str) -> String {
    let trimmed = content.trim();
    if let Some(open) = trimmed.find("```") {
        // Skip the opening fence and its language tag (e.g. "json\n").
        let after_open = &trimmed[open + 3..];
        let after_lang = match after_open.find('\n') {
            Some(nl) => &after_open[nl + 1..],
            None => after_open,
        };
        if let Some(end) = after_lang.find("```") {
            return after_lang[..end].trim().to_string();
        }
    }
    trimmed.to_string()
}
