use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    pub internal_date: String,
    #[serde(default)]
    pub size_estimate: u32,
    pub payload: Option<MessagePayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePayload {
    pub mime_type: String,
    #[serde(default)]
    pub headers: Vec<Header>,
    pub body: Option<Body>,
    #[serde(default)]
    pub parts: Option<Vec<MessagePayload>>,
    #[serde(default)]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Body {
    pub size: u32,
    pub data: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub label_type: String,
    #[serde(default)]
    pub message_list_visibility: Option<String>,
    #[serde(default)]
    pub label_list_visibility: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailProfile {
    pub email_address: String,
    pub messages_total: u64,
    pub threads_total: u64,
    pub history_id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListMessagesResponse {
    #[serde(default)]
    pub messages: Option<Vec<MessageRef>>,
    #[serde(default)]
    pub next_page_token: Option<String>,
    #[serde(default)]
    pub result_size_estimate: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListLabelsResponse {
    pub labels: Vec<Label>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ListMessagesRequest<'a> {
    pub q: &'a str,
    pub max_results: u32,
}

#[derive(Debug, Serialize)]
pub(crate) struct ModifyLabelsRequest<'a> {
    pub add_label_ids: &'a [&'a str],
    pub remove_label_ids: &'a [&'a str],
}

#[derive(Debug, Serialize)]
pub(crate) struct BatchModifyRequest<'a> {
    pub ids: &'a [&'a str],
    pub add_label_ids: &'a [&'a str],
    pub remove_label_ids: &'a [&'a str],
}

#[derive(Debug, Serialize)]
pub(crate) struct CreateLabelRequest<'a> {
    pub name: &'a str,
    pub message_list_visibility: &'a str,
    pub label_list_visibility: &'a str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    pub expires_in: u32,
    #[serde(default)]
    pub token_type: String,
}
