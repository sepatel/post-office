# Gmail API Integration

## Design Decision: Thin REST Wrapper

We build our own thin Gmail REST API wrapper using `reqwest` instead of depending on `google-gmail1` (deprecated) or `gmail` (libninja, low adoption, non-standard HTTP client).

**Rationale:**
- Full control over HTTP client and error handling
- No dependency risk on unmaintained crates
- Can evolve with Gmail API changes immediately
- Standard `reqwest` ecosystem compatibility
- Transparent token refresh and rate limiting

## Authentication

### OAuth2 Scopes

```
https://www.googleapis.com/auth/gmail.modify    # Read + label modify + trash + spam
https://www.googleapis.com/auth/gmail.labels    # Create/update/delete labels
```

These scopes cover all operations we need:
- Read messages (list, get)
- Modify labels (add/remove — handles archive, trash, spam, custom labels)
- Create labels for organization

### OAuth2 Flow

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   Desktop    │────>│   Browser    │────>│   Google     │
│   App        │     │   (OAuth2)   │     │   Consent    │
│              │<────│              │<────│   Screen     │
└──────┬───────┘     └──────────────┘     └──────────────┘
       │
       │ Redirect to localhost with auth code
       ▼
┌──────────────┐
│  Exchange    │
│  code for    │
│  tokens      │
└──────┬───────┘
       │
       ▼
┌──────────────┐
│  Store in    │
│  keyring     │
│  (OS-native) │
└──────────────┘
```

### Token Management

```rust
pub struct GmailAuth {
    access_token: String,
    refresh_token: String,
    client_id: String,
    client_secret: String,
    expires_at: std::time::Instant,
}

impl GmailAuth {
    pub fn is_expired(&self) -> bool {
        self.expires_at <= std::time::Instant::now() + std::time::Duration::from_secs(300)
    }

    pub async fn refresh(&mut self, http: &reqwest::Client) -> Result<(), GmailError> {
        let token: TokenResponse = http
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", self.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await?
            .json()
            .await?;

        self.access_token = token.access_token;
        self.expires_at =
            std::time::Instant::now() + std::time::Duration::from_secs(token.expires_in.into());

        Ok(())
    }
}
```

### Token Storage

Tokens are stored in the OS keychain via the `keyring` crate:
- **macOS**: Keychain
- **Windows**: Credential Manager
- **Linux**: Secret Service (GNOME Keyring, KWallet)

```rust
use keyring::Entry;

#[derive(Serialize, Deserialize)]
struct TokenStore {
    access: String,
    refresh: String,
}

fn store_tokens(account: &str, access: &str, refresh: &str) -> keyring::Result<()> {
    let entry = Entry::new("post-office", account)?;
    let tokens = TokenStore {
        access: access.to_string(),
        refresh: refresh.to_string(),
    };
    entry.set_password(&serde_json::to_string(&tokens)?)
}

fn load_tokens(account: &str) -> keyring::Result<Option<TokenStore>> {
    let entry = Entry::new("post-office", account)?;
    match entry.get_password() {
        Ok(json) => serde_json::from_str(&json).map(Some).map_err(Into::into),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e),
    }
}
```

## REST API Client

### Base Client

```rust
// crates/core/src/gmail/client.rs

use reqwest::Client;
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://gmail.googleapis.com/gmail/v1";

pub struct GmailClient {
    http: Client,
    auth: GmailAuth,
}

impl GmailClient {
    pub fn new(auth: GmailAuth) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    async fn request<T: DeserializeOwned>(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<impl Serialize>,
    ) -> Result<T, GmailError> {
        if self.auth.is_expired() {
            self.auth.refresh(&self.http).await?;
        }

        let url = format!("{BASE_URL}/users/me{path}");
        let mut req = self.http.request(method, &url).bearer_auth(&self.auth.access_token);

        if let Some(body) = body {
            req = req.json(&body);
        }

        let response = req.send().await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.auth.refresh(&self.http).await?;
            let mut retry_req = self
                .http
                .request(method, &url)
                .bearer_auth(&self.auth.access_token);
            if let Some(body) = body {
                retry_req = retry_req.json(&body);
            }
            return retry_req.send().await?.json().await;
        }

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or(60);

            tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
            return Box::pin(self.request(method, path, body)).await;
        }

        response.json().await
    }
}
```

### Message Operations

```rust
// crates/core/src/gmail/messages.rs

#[derive(Serialize)]
struct ListMessagesRequest<'a> {
    q: &'a str,
    max_results: u32,
}

#[derive(Serialize)]
struct ModifyLabelsRequest<'a> {
    add_label_ids: &'a [&'a str],
    remove_label_ids: &'a [&'a str],
}

#[derive(Serialize)]
struct BatchModifyRequest<'a> {
    ids: &'a [&'a str],
    add_label_ids: &'a [&'a str],
    remove_label_ids: &'a [&'a str],
}

impl GmailClient {
    pub async fn list_messages(
        &mut self,
        query: &str,
        max_results: u32,
    ) -> Result<Vec<MessageRef>, GmailError> {
        let response: ListMessagesResponse = self
            .request(
                reqwest::Method::GET,
                &format!("/messages?maxResults={max_results}"),
                Some(ListMessagesRequest { q: query, max_results }),
            )
            .await?;

        Ok(response.messages.unwrap_or_default())
    }

    pub async fn get_message(&mut self, id: &str, format: &str) -> Result<Message, GmailError> {
        self.request(
            reqwest::Method::GET,
            &format!("/messages/{id}?format={format}"),
            None::<()>,
        )
        .await
    }

    pub async fn modify_labels(
        &mut self,
        id: &str,
        add_label_ids: &[&str],
        remove_label_ids: &[&str],
    ) -> Result<Message, GmailError> {
        self.request(
            reqwest::Method::POST,
            &format!("/messages/{id}/modify"),
            Some(ModifyLabelsRequest { add_label_ids, remove_label_ids }),
        )
        .await
    }

    pub async fn batch_modify(
        &mut self,
        ids: &[&str],
        add_label_ids: &[&str],
        remove_label_ids: &[&str],
    ) -> Result<(), GmailError> {
        for chunk in ids.chunks(1000) {
            self.request::<()>(
                reqwest::Method::POST,
                "/messages/batchModify",
                Some(BatchModifyRequest { ids: chunk, add_label_ids, remove_label_ids }),
            )
            .await?;
        }
        Ok(())
    }
}
```

### Label Operations

```rust
// crates/core/src/gmail/labels.rs

#[derive(Serialize)]
struct CreateLabelRequest<'a> {
    name: &'a str,
    message_list_visibility: &'a str,
    label_list_visibility: &'a str,
}

impl GmailClient {
    pub async fn list_labels(&mut self) -> Result<Vec<Label>, GmailError> {
        let response: ListLabelsResponse = self
            .request(reqwest::Method::GET, "/labels", None::<()>)
            .await?;

        Ok(response.labels)
    }

    pub async fn create_label(
        &mut self,
        name: &str,
        visibility: LabelVisibility,
    ) -> Result<Label, GmailError> {
        let (mlv, llv) = match visibility {
            LabelVisibility::Show => ("show", "labelShow"),
            LabelVisibility::Hide => ("hide", "labelHide"),
            LabelVisibility::ShowIfUnread => ("show", "labelShowIfUnread"),
        };

        self.request(
            reqwest::Method::POST,
            "/labels",
            Some(CreateLabelRequest {
                name,
                message_list_visibility: mlv,
                label_list_visibility: llv,
            }),
        )
        .await
    }
}
```

## Action Mappings

How Gmail actions map to API calls:

| Action | `addLabelIds` | `removeLabelIds` |
|--------|---------------|-------------------|
| Archive | — | `["INBOX"]` |
| Trash | `["TRASH"]` | `["INBOX"]` |
| Spam | `["SPAM"]` | `["INBOX"]` |
| Mark read | — | `["UNREAD"]` |
| Mark unread | `["UNREAD"]` | — |
| Star | `["STARRED"]` | — |
| Apply label | `["Label_xxxxx"]` | — |
| Remove label | — | `["Label_xxxxx"]` |

`APPLY` is a rules-engine control token, not a Gmail action. The engine resolves
`APPLY` to the rule's configured structured actions before calling
`modify_labels`.

## Rate Limiting

### Quota Costs

| Operation | Quota units |
|-----------|-------------|
| `messages.list` | 5 |
| `messages.get` | 5 |
| `messages.modify` | 5 |
| `messages.batchModify` | 5 |
| `messages.trash` / `untrash` | 10 |
| `labels.list` | 1 |
| `labels.create` | 5 |

### Rate Limit Strategy

- **Per-user limit**: 6,000 quota units/minute
- **Client-side enforcement**: Track quota usage, sleep if approaching limit
- **Batch operations**: Use `batchModify` for bulk label changes (5 units for up to 1000 messages)
- **Retry on 429**: Exponential backoff with jitter, respect `Retry-After` header

```rust
pub struct RateLimiter {
    quota_used: u32,
    quota_limit: u32,
    window_start: std::time::Instant,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self {
            quota_used: 0,
            quota_limit: 6000,
            window_start: std::time::Instant::now(),
        }
    }
}

impl RateLimiter {
    pub async fn acquire(&mut self, cost: u32) {
        if self.window_start.elapsed() > std::time::Duration::from_secs(60) {
            self.quota_used = 0;
            self.window_start = std::time::Instant::now();
        }

        if self.quota_used + cost > self.quota_limit {
            let wait = std::time::Duration::from_secs(60) - self.window_start.elapsed();
            tokio::time::sleep(wait).await;
            self.quota_used = 0;
            self.window_start = std::time::Instant::now();
        }

        self.quota_used += cost;
    }
}
```

## Gmail Search Query Syntax

The `q` parameter in `messages.list` uses Gmail's search syntax:

```
from:sender@example.com
to:recipient@example.com
subject:keyword
has:attachment
filename:report.pdf
is:unread
is:starred
is:important
label:INBOX
label:Label_123
after:2024/01/01
before:2024/12/31
newer_than:7d
older_than:30d
category:promotions
{exact phrase}
-(exclude this)
larger:10M
smaller:1M
```

Queries can be combined with spaces (AND) and `-` (NOT).

## Push + Cursor Sync Endpoints

Target-state ingestion uses Gmail push notifications plus cursor replay:

- `POST /users/me/watch`
- `POST /users/me/stop`
- `GET /users/me/history?startHistoryId=...`

Design constraints:

- Notifications only carry `emailAddress` + `historyId`; they are a trigger, not
  complete mailbox state.
- `history.list` is the source of truth and must be paged until caught up.
- Watches expire and must be renewed (Gmail max is 7 days).
- Desktop apps cannot expose public webhooks directly, so Pub/Sub push requires
  a cloud relay.

See `design/gmail-push-sync.md` for full protocol, relay architecture, and
failure-recovery behavior.

## Models

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRef {
    pub id: String,
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub thread_id: String,
    pub label_ids: Vec<String>,
    pub snippet: String,
    pub history_id: String,
    pub internal_date: String,  // Epoch millis as string
    pub size_estimate: u32,
    pub payload: Option<MessagePayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePayload {
    pub mime_type: String,
    pub headers: Vec<Header>,
    pub body: Option<Body>,
    pub parts: Option<Vec<MessagePayload>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Body {
    pub size: u32,
    pub data: Option<String>,  // Base64url-encoded
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub label_type: String,  // "system" or "user"
    pub message_list_visibility: Option<String>,
    pub label_list_visibility: Option<String>,
}
```

## Error Handling

```rust
#[derive(Debug, thiserror::Error)]
pub enum GmailError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("rate limited, retry after {0}s")]
    RateLimited(u64),

    #[error("API error: {code} - {message}")]
    Api { code: u16, message: String },

    #[error("token refresh failed")]
    TokenRefresh,
}
```
