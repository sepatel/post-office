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
        if self.auth.is_expired() {
            self.auth.refresh(&self.http).await?;
        }

        let url = format!("{BASE_URL}/users/me{path}");
        let mut req = self.http.request(method.clone(), &url).bearer_auth(&self.auth.access_token);

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
            return retry_req.send().await?.json().await.map_err(GmailError::from);
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

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(GmailError::Api {
                code: status.as_u16(),
                message: body,
            });
        }

        response.json().await.map_err(GmailError::from)
    }

    pub async fn list_messages(
        &mut self,
        query: &str,
        max_results: u32,
    ) -> Result<Vec<MessageRef>, GmailError> {
        let request_body = ListMessagesRequest {
            q: query,
            max_results,
        };
        let response: ListMessagesResponse = self
            .request(
                reqwest::Method::GET,
                &format!("/messages?maxResults={max_results}"),
                Some(&request_body),
            )
            .await?;

        Ok(response.messages.unwrap_or_default())
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
    ) -> Result<Message, GmailError> {
        let request_body = ModifyLabelsRequest {
            add_label_ids,
            remove_label_ids,
        };
        self.request(
            reqwest::Method::POST,
            &format!("/messages/{id}/modify"),
            Some(&request_body),
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
            let request_body = BatchModifyRequest {
                ids: chunk,
                add_label_ids,
                remove_label_ids,
            };
            self.request::<()>(
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
        self.request(
            reqwest::Method::POST,
            "/labels",
            Some(&request_body),
        )
        .await
    }

    pub async fn get_profile(&mut self) -> Result<GmailProfile, GmailError> {
        self.request(reqwest::Method::GET, "/profile", None::<&()>)
            .await
    }
}
