use async_openai::{config::OpenAIConfig, Client};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
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
        if let Some(reasoning_effort) = request.reasoning_effort {
            body["reasoning_effort"] = json!(reasoning_effort);
            if request.litellm_reasoning_passthrough {
                body["allowed_openai_params"] = json!(["reasoning_effort"]);
            }
        }

        body["stream"] = json!(true);
        let mut stream = self
            .client
            .chat()
            .create_stream_byot::<Value, Value>(body)
            .await?;
        let mut content = String::new();
        let mut finish_reason = None;
        let mut has_reasoning = false;
        let mut prompt_tokens = None;
        let mut completion_tokens = None;
        let mut tokens_used = None;
        let mut model_used = None;
        let mut request_key = None;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            model_used = chunk["model"].as_str().map(str::to_string).or(model_used);
            request_key = chunk["id"].as_str().map(str::to_string).or(request_key);
            let choice = chunk["choices"]
                .as_array()
                .and_then(|choices| choices.first());
            if let Some(choice) = choice {
                let delta = &choice["delta"];
                content.push_str(&response_content(delta));
                has_reasoning |= ["reasoning", "reasoning_content", "reasoning_details"]
                    .into_iter()
                    .any(|field| delta[field].is_array() || delta[field].is_string());
                finish_reason = choice["finish_reason"]
                    .as_str()
                    .map(str::to_string)
                    .or(finish_reason);
            }
            prompt_tokens = chunk["usage"]["prompt_tokens"]
                .as_u64()
                .map(|value| value as u32)
                .or(prompt_tokens);
            completion_tokens = chunk["usage"]["completion_tokens"]
                .as_u64()
                .map(|value| value as u32)
                .or(completion_tokens);
            tokens_used = chunk["usage"]["total_tokens"]
                .as_u64()
                .map(|value| value as u32)
                .or(tokens_used);
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(ProcessResponse {
            content,
            finish_reason,
            has_reasoning,
            model: model_used.unwrap_or(model),
            provider_id: self.provider_id.clone(),
            prompt_tokens,
            completion_tokens,
            tokens_used,
            duration_ms,
            request_key: request_key.unwrap_or_else(|| {
                format!("local-{}", REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed))
            }),
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
        self.test_connection_with_reasoning(None).await
    }

    pub async fn test_connection_with_reasoning(
        &self,
        reasoning_effort: Option<&str>,
    ) -> Result<ProcessResponse, LlmError> {
        let mut request = ProcessRequest {
            system_prompt: Some("Reply with the single word OK.".into()),
            user_prompt: "Test.".into(),
            model: None,
            temperature: Some(0.0),
            max_tokens: Some(5),
            kind: super::ProcessKind::Health,
            reasoning_effort: reasoning_effort.map(str::to_string),
            litellm_reasoning_passthrough: false,
        };
        match self.process(request.clone()).await {
            Err(error)
                if request.reasoning_effort.is_some()
                    && error.requires_litellm_reasoning_passthrough() =>
            {
                request.litellm_reasoning_passthrough = true;
                self.process(request).await
            }
            response => response,
        }
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

fn response_content(message: &Value) -> String {
    if let Some(content) = message["content"].as_str() {
        return content.to_string();
    }
    message["content"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part["text"].as_str().or_else(|| part.as_str()))
                .collect::<String>()
        })
        .unwrap_or_default()
}

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_parts_from_openai_compatible_responses() {
        let message = json!({
            "content": [
                {"type": "text", "text": "MATCH"},
                {"type": "text", "text": " | relevant"}
            ]
        });

        assert_eq!(response_content(&message), "MATCH | relevant");
    }
}
