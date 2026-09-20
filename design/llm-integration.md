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
// crates/core/src/rules/engine.rs (simplified)

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

Every decision request contains exactly one email and receives one decision:

```text
1: "Invoices" | Billing statement from the bank.
```

The answer is a choice from the rule's menu, or `NO_MATCH`, which advances to
the next rule. With no menu the only choices are `MATCH` and `NO_MATCH`.

### Leniency

Parsing is lenient wherever the meaning is unambiguous, because refusing a row
costs a re-ask:

- A name outside the menu is dropped and reported as a non-fatal note.
- A selection made against an empty menu is vacuous and ignored.
- A bare action token with no menu counts as a match; the token is discarded so
  the rule's own actions decide the effect.
- `MATCH | CHOOSE: x`, `MATCH | LABELS: x`, and a bare `x` all read the same,
  as does the same content spread over several lines.

A line that opens with prose rather than a choice is not an answer, so a
preamble or trailing summary is skipped rather than miscounted.

### Choices

A rule's menu is `choices`, a list of actions the model picks between; `actions`
is the recipe that runs on any match. Because a choice is an action, a label and
"move to trash" are the same kind of thing to the model, so "file it under A, B,
or bin it" is one rule rather than two modes.

| Menu | On match |
| --- | --- |
| empty | a plain match-or-not question; `actions` run |
| `choices` | the chosen action runs, plus `actions` |
| `choose_from_all_labels` | every Gmail user label is offered; the chosen label is added to `actions` |

Choices are always referenced by name; a positional index is only meaningful
within the request that produced it, so resolving a stray number would silently
pick whatever choice sat at that offset. `choose_from_all_labels` sends every
label name with every email, so it costs prompt budget proportional to the
mailbox.

### Label Cache

Labels are cached per account in `gmail_labels` (id, name, type as a JSON blob
plus `fetched_at`) and refreshed on read once older than the staleness window.
The app never creates labels, so nothing invalidates the cache out of band.

A failed refresh degrades to the stale copy rather than an empty list: an empty
catalog silently strips label actions and makes execution fail with "unknown
label". Only a failure with nothing cached at all surfaces as an error.

### Context Budgeting

Each provider profile records its context window. Rules reserve completion
tokens and size a routing policy against its smallest eligible provider. When
the email body exceeds the remaining budget, the prompt keeps headers plus the
beginning and end of the body and marks the omitted middle.

### Prompt Template

The contract and the menu live in the system prompt, which is built per rule.
The payload carries only what varies per request:

```text
RULE INSTRUCTION:
{user's rule prompt}

--- Learned Memory ---
{exceptions and notes}

EMAIL:
From: sender@example.com
Subject: Check in

Hey, just wanted to check in about...

Answer with exactly one decision line.
```

### Parser

`rules::engine::parse_row` reads the response into a decision. It is small
enough to read directly, and the copy that used to live here went stale, so it
is not reproduced.

The shape: strip the leading marker, split on `|` and newlines, take the decline
keyword off the head if present, otherwise resolve the selection against the
menu by name.

### Why This Format

- **Isolation**: a model response can affect only the email it was given
- **Predictability**: a response is a decision or it is not
- **Debugging**: the row that produced a decision is stored on the history entry
- **Simplicity**: one contract, one prompt, and one parser

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
| Timeout or transport failure | Configurable provider deadline, record the failed attempt and retry through the durable queue |
| Rate limit | Exponential backoff, respect 429 headers |
| Malformed response | Log full response, skip email, mark as `invalid_format` in history |
| Invalid API key | Notify user, pause processing |
