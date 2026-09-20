use reqwest::Client;
use serde::de::DeserializeOwned;

use super::auth::GmailAuth;
use super::models::*;
use crate::GmailError;

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
        body: Option<&(impl serde::Serialize + Send)>,
    ) -> Result<T, GmailError> {
        let response = self.send(method, path, body).await?;

        // Some Gmail mutations (e.g. modify/batchModify) can reply with no body.
        // Treat an empty body as `null` so callers that don't need the payload
        // (typed as `()` / `Option` / `Value`) still decode cleanly.
        let text = response.text().await.unwrap_or_default();
        let text = if text.trim().is_empty() {
            "null"
        } else {
            text.as_str()
        };
        serde_json::from_str::<T>(text).map_err(GmailError::from)
    }

    async fn send(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<&(impl serde::Serialize + Send)>,
    ) -> Result<reqwest::Response, GmailError> {
        if self.auth.is_expired() {
            self.auth.refresh(&self.http).await?;
        }

        let url = format!("{BASE_URL}/users/me{path}");
        let mut req = self
            .http
            .request(method.clone(), &url)
            .bearer_auth(&self.auth.access_token);

        if let Some(body) = body {
            req = req.json(body);
        }

        let response = req.send().await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.auth.refresh(&self.http).await?;
            let mut retry_req = self
                .http
                .request(method, &url)
                .bearer_auth(&self.auth.access_token);
            if let Some(body) = body {
                retry_req = retry_req.json(body);
            }
            return retry_req.send().await.map_err(GmailError::from);
        }

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or(60);

            tracing::warn!(
                path,
                retry_after,
                "Gmail rate limited; waiting before retry"
            );
            tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
            return Box::pin(self.send(method, path, body)).await;
        }

        if response.status() == reqwest::StatusCode::FORBIDDEN {
            let body_text = response.text().await.unwrap_or_default();
            if is_rate_limit_error(&body_text) {
                tracing::warn!(path, "Gmail quota exceeded; waiting before retry");
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                return Box::pin(self.send(method, path, body)).await;
            }
            return Err(GmailError::Api {
                code: reqwest::StatusCode::FORBIDDEN.as_u16(),
                message: body_text,
            });
        }

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(GmailError::Api {
                code: status.as_u16(),
                message: body,
            });
        }

        Ok(response)
    }

    pub async fn list_messages(
        &mut self,
        query: &str,
        max_results: u32,
    ) -> Result<Vec<MessageRef>, GmailError> {
        if max_results == 0 {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        let mut page_token: Option<String> = None;

        while out.len() < max_results as usize {
            let remaining = (max_results as usize).saturating_sub(out.len()) as u32;
            let page = self
                .list_messages_page(query, remaining.min(500), page_token.as_deref())
                .await?;
            out.extend(page.messages.unwrap_or_default());

            if out.len() >= max_results as usize {
                break;
            }

            page_token = page.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        Ok(out)
    }

    pub async fn list_messages_page(
        &mut self,
        query: &str,
        max_results: u32,
        page_token: Option<&str>,
    ) -> Result<ListMessagesResponse, GmailError> {
        let qs = {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            serializer.append_pair("q", query);
            if let Some(token) = page_token {
                serializer.append_pair("pageToken", token);
            }
            serializer.finish()
        };
        let path = format!("/messages?maxResults={max_results}&{qs}");
        self.request(reqwest::Method::GET, &path, None::<&()>).await
    }

    pub async fn get_message(&mut self, id: &str) -> Result<Message, GmailError> {
        self.request(
            reqwest::Method::GET,
            &format!("/messages/{id}?format=full"),
            None::<&()>,
        )
        .await
    }

    pub async fn modify_labels(
        &mut self,
        id: &str,
        add_label_ids: &[&str],
        remove_label_ids: &[&str],
    ) -> Result<(), GmailError> {
        let request_body = ModifyLabelsRequest {
            add_label_ids,
            remove_label_ids,
        };
        // Decode modify responses as JSON values; Gmail returns a Message object,
        // so deserializing into `()` fails.
        self.request::<serde_json::Value>(
            reqwest::Method::POST,
            &format!("/messages/{id}/modify"),
            Some(&request_body),
        )
        .await?;
        Ok(())
    }

    pub async fn batch_modify(
        &mut self,
        ids: &[&str],
        add_label_ids: &[&str],
        remove_label_ids: &[&str],
    ) -> Result<(), GmailError> {
        for chunk in ids.chunks(1000) {
            let request_body = BatchModifyRequest {
                ids: chunk,
                add_label_ids,
                remove_label_ids,
            };
            self.request::<serde_json::Value>(
                reqwest::Method::POST,
                "/messages/batchModify",
                Some(&request_body),
            )
            .await?;
        }
        Ok(())
    }

    pub async fn list_labels(&mut self) -> Result<Vec<Label>, GmailError> {
        let response: ListLabelsResponse = self
            .request(reqwest::Method::GET, "/labels", None::<&()>)
            .await?;

        Ok(response.labels)
    }

    pub async fn create_label(&mut self, name: &str) -> Result<Label, GmailError> {
        let request_body = CreateLabelRequest {
            name,
            message_list_visibility: "show",
            label_list_visibility: "labelShow",
        };
        self.request(reqwest::Method::POST, "/labels", Some(&request_body))
            .await
    }

    pub async fn get_profile(&mut self) -> Result<GmailProfile, GmailError> {
        self.request(reqwest::Method::GET, "/profile", None::<&()>)
            .await
    }

    pub async fn watch(
        &mut self,
        topic_name: &str,
        label_ids: &[String],
    ) -> Result<WatchResponse, GmailError> {
        let request_body = WatchRequest {
            topic_name,
            label_ids: label_ids.to_vec(),
            label_filter_behavior: if label_ids.is_empty() {
                None
            } else {
                Some("include")
            },
        };

        self.request(reqwest::Method::POST, "/watch", Some(&request_body))
            .await
    }

    pub async fn stop_watch(&mut self) -> Result<(), GmailError> {
        self.request::<serde_json::Value>(
            reqwest::Method::POST,
            "/stop",
            Some(&serde_json::json!({})),
        )
        .await?;
        Ok(())
    }

    pub async fn list_history_page(
        &mut self,
        start_history_id: &str,
        page_token: Option<&str>,
    ) -> Result<ListHistoryResponse, GmailError> {
        let qs = {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            serializer.append_pair("startHistoryId", start_history_id);
            serializer.append_pair("maxResults", "500");
            if let Some(token) = page_token {
                serializer.append_pair("pageToken", token);
            }
            serializer.finish()
        };
        let path = format!("/history?{qs}");
        self.request(reqwest::Method::GET, &path, None::<&()>).await
    }
}

fn is_rate_limit_error(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.pointer("/error/errors")?.as_array().cloned())
        .is_some_and(|errors| {
            errors.iter().any(|error| {
                matches!(
                    error.get("reason").and_then(serde_json::Value::as_str),
                    Some("rateLimitExceeded" | "userRateLimitExceeded")
                )
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_google_quota_errors() {
        assert!(is_rate_limit_error(
            r#"{"error":{"errors":[{"reason":"rateLimitExceeded"}]}}"#
        ));
        assert!(is_rate_limit_error(
            r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#
        ));
        assert!(!is_rate_limit_error(
            r#"{"error":{"errors":[{"reason":"forbidden"}]}}"#
        ));
    }
}
