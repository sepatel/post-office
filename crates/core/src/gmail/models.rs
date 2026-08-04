use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRef {
    pub id: String,
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
pub struct GmailProfile {
    pub email_address: String,
    pub messages_total: u64,
    pub threads_total: u64,
    pub history_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMessagesResponse {
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
#[serde(rename_all = "camelCase")]
pub(crate) struct ModifyLabelsRequest<'a> {
    pub add_label_ids: &'a [&'a str],
    pub remove_label_ids: &'a [&'a str],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BatchModifyRequest<'a> {
    pub ids: &'a [&'a str],
    pub add_label_ids: &'a [&'a str],
    pub remove_label_ids: &'a [&'a str],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateLabelRequest<'a> {
    pub name: &'a str,
    pub message_list_visibility: &'a str,
    pub label_list_visibility: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GmailConnection {
    pub connected: bool,
    pub email: Option<String>,
    pub messages_total: Option<u64>,
    pub threads_total: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub expires_in: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WatchRequest<'a> {
    pub topic_name: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub label_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_filter_behavior: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchResponse {
    pub history_id: String,
    pub expiration: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListHistoryResponse {
    #[serde(default)]
    pub history: Option<Vec<HistoryRecord>>,
    #[serde(default)]
    pub next_page_token: Option<String>,
    #[serde(default)]
    pub history_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub id: String,
    #[serde(default)]
    pub messages: Option<Vec<MessageRef>>,
    #[serde(default)]
    pub messages_added: Option<Vec<HistoryMessageEnvelope>>,
    #[serde(default)]
    pub messages_deleted: Option<Vec<HistoryMessageEnvelope>>,
    #[serde(default)]
    pub labels_added: Option<Vec<HistoryLabelEnvelope>>,
    #[serde(default)]
    pub labels_removed: Option<Vec<HistoryLabelEnvelope>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessageEnvelope {
    pub message: MessageRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryLabelEnvelope {
    pub message: MessageRef,
}
