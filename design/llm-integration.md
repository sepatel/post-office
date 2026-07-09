# LLM Integration

## Design

Uses `async-openai` to communicate with any OpenAI-compatible endpoint. Users configure the base URL to point to their local LLM server (Ollama, llama.cpp, vLLM, LM Studio, etc.).

## Client

```rust
// crates/core/src/llm/client.rs

use async_openai::{
    Client,
    config::OpenAIConfig,
    types::{ChatCompletionRequestMessage, CreateChatCompletionRequest, Role},
};

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
        let model = request.model.unwrap_or_else(|| self.default_model.clone());

        let mut messages = Vec::new();

        if let Some(system_prompt) = &request.system_prompt {
            messages.push(ChatCompletionRequestMessage {
                role: Role::System,
                content: Some(system_prompt.clone()),
                ..Default::default()
            });
        }

        messages.push(ChatCompletionRequestMessage {
            role: Role::User,
            content: Some(request.user_prompt),
            ..Default::default()
        });

        let start = std::time::Instant::now();

        let response = self.client
            .chat()
            .create(CreateChatCompletionRequest {
                model,
                messages,
                temperature: request.temperature,
                max_tokens: request.max_tokens,
                ..Default::default()
            })
            .await?;

        let duration_ms = start.elapsed().as_millis() as u64;

        let choice = response.choices.first()
            .ok_or(LlmError::NoResponse)?;

        let content = choice.message.content.clone()
            .unwrap_or_default();

        let tokens_used = response.usage
            .map(|u| u.total_tokens as u32);

        Ok(ProcessResponse {
            content,
            model: response.model,
            tokens_used,
            duration_ms,
        })
    }
}
```

## Request/Response Types

```rust
// crates/core/src/llm/models.rs

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
            user_prompt: user_prompt.into(),
            ..Self::default()
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

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(#[from] async_openai::error::OpenAIError),

    #[error("No response from LLM")]
    NoResponse,

    #[error("Endpoint unreachable: {0}")]
    Unreachable(String),
}
```

## Prompt Construction

How emails are formatted for the LLM:

```rust
// crates/core/src/llm/prompts.rs

use crate::gmail::models::Message;

pub fn build_user_prompt(rule_prompt: &str, email: &Message) -> String {
    let headers = email
        .payload
        .as_ref()
        .and_then(|p| p.headers.as_ref())
        .map(|h| {
            h.iter()
                .map(|header| format!("{}: {}", header.name, header.value))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    let body = extract_plain_text(email);

    format!(
        "--- Email Headers ---\n\
         {headers}\n\n\
         --- Email Body ---\n\
         {body}\n\n\
         --- Rule Instruction ---\n\
         {rule_prompt}"
    )
}

fn extract_plain_text(email: &Message) -> String {
    let payload = match &email.payload {
        Some(p) => p,
        None => return email.snippet.clone(),
    };

    // Prefer text/plain for cleaner LLM input
    let find_plain = |parts: &[MessagePayload]| -> Option<String> {
        parts
            .iter()
            .find(|p| p.mime_type == "text/plain")
            .and_then(|p| p.body.as_ref())
            .and_then(|b| b.data.as_deref())
            .map(base64url_decode)
    };

    let find_html = |parts: &[MessagePayload]| -> Option<String> {
        parts
            .iter()
            .find(|p| p.mime_type == "text/html")
            .and_then(|p| p.body.as_ref())
            .and_then(|b| b.data.as_deref())
            .map(|data| html_to_text(&base64url_decode(data)))
    };

    // Try direct body first, then parts
    payload
        .body
        .as_ref()
        .and_then(|b| b.data.as_deref())
        .filter(|_| payload.mime_type == "text/plain")
        .map(base64url_decode)
        .or_else(|| payload.parts.as_deref().and_then(&find_plain))
        .or_else(|| payload.parts.as_deref().and_then(&find_html))
        .unwrap_or_else(|| email.snippet.clone())
}

fn base64url_decode(data: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE
        .decode(data)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn html_to_text(html: &str) -> String {
    // Compiled once via LazyLock — regex compilation is expensive
    use std::sync::LazyLock;
    static TAG_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"<[^>]+>").unwrap());

    TAG_RE
        .replace_all(html, "")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}
```

## Response Parsing

The LLM is prompted to respond in a strict format. If the response doesn't match, the email is skipped.

### Prompt Template

The user prompt is constructed as:

```
--- Email Headers ---
From: sender@example.com
To: recipient@example.com
Subject: Check in

--- Email Body ---
Hey, just wanted to check in about...

--- Rule Instruction ---
{user's rule prompt}

--- Response Format ---
You MUST respond with EXACTLY ONE of these lines:
- ARCHIVE
- TRASH
- SPAM
- LABEL: <name>
- MARK_READ
- MARK_UNREAD
- STAR
- SKIP

No other text. Just the action word.
If you need a label, use: LABEL: <name>
```

### Parser

```rust
// crates/core/src/rules/response_parser.rs

use std::str::FromStr;

/// Parse a strict-format LLM response into a single action.
/// Returns None if the response doesn't match the expected format (skip the email).
pub fn parse_llm_response(response: &str) -> Option<ParsedAction> {
    let line = response.trim();

    // Handle multi-line responses: take the first non-empty line
    let first_line = line.lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())?;

    ParsedAction::from_str(first_line).ok()
}

/// A single action parsed from the LLM response.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedAction {
    Archive,
    Trash,
    Spam,
    MarkRead,
    MarkUnread,
    Star,
    Label(String),
}

impl FromStr for ParsedAction {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        if s.eq_ignore_ascii_case("ARCHIVE") {
            return Ok(ParsedAction::Archive);
        }
        if s.eq_ignore_ascii_case("TRASH") {
            return Ok(ParsedAction::Trash);
        }
        if s.eq_ignore_ascii_case("SPAM") {
            return Ok(ParsedAction::Spam);
        }
        if s.eq_ignore_ascii_case("MARK_READ") {
            return Ok(ParsedAction::MarkRead);
        }
        if s.eq_ignore_ascii_case("MARK_UNREAD") {
            return Ok(ParsedAction::MarkUnread);
        }
        if s.eq_ignore_ascii_case("STAR") {
            return Ok(ParsedAction::Star);
        }
        if s.eq_ignore_ascii_case("SKIP") {
            return Err(ParseError::Skip);
        }

        // LABEL: <name>
        s.strip_prefix("LABEL:")
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .map(|name| ParsedAction::Label(name.to_string()))
            .ok_or(ParseError::InvalidFormat)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("LLM response does not match expected format")]
    InvalidFormat,

    #[error("LLM responded with SKIP")]
    Skip,
}
```

### Why Strict Format

- **Cost efficiency**: No retry loops, no wasted tokens on malformed responses
- **Predictability**: Every response is either valid or skipped, no ambiguity
- **Debugging**: When an email is skipped, the full LLM response is logged for review
- **Simplicity**: The parser is ~30 lines of code with no regex, no keyword heuristics

## Configuration

LLM settings stored in config table:

```json
{
  "llm.base_url": "http://localhost:11434/v1",
  "llm.api_key": "ollama",
  "llm.default_model": "llama3",
  "llm.temperature": "0.3",
  "llm.max_tokens": "1024"
}
```

### Supported Endpoints

| Endpoint | Base URL | Notes |
|----------|----------|-------|
| Ollama | `http://localhost:11434/v1` | Default, no API key needed |
| llama.cpp | `http://localhost:8080/v1` | Server mode |
| vLLM | `http://localhost:8000/v1` | High throughput |
| LM Studio | `http://localhost:1234/v1` | GUI-based |
| OpenAI | `https://api.openai.com/v1` | Cloud fallback |
| OpenRouter | `https://openrouter.ai/api/v1` | Multi-model proxy |

## Error Handling

| Error | Handling |
|-------|----------|
| Endpoint unreachable | Log error, skip email, mark as failed in history |
| Timeout | Configurable timeout (default 30s), skip email |
| Rate limit | Exponential backoff, respect 429 headers |
| Malformed response | Log full response, skip email, mark as `invalid_format` in history |
| Invalid API key | Notify user, pause processing |
