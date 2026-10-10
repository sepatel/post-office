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
    #[serde(default)]
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

/// Gmail system labels that are mailbox state, not user topics. They must not
/// appear as classifier choices or in the global label library — asking a user
/// to describe `INBOX` or `CATEGORY_PROMOTIONS` interweaves Google's fixed
/// taxonomy into their own labels. They remain usable in conditions.
const EXCLUDED_CLASSIFICATION_IDS: [&str; 14] = [
    "INBOX",
    "SPAM",
    "TRASH",
    "DRAFT",
    "SENT",
    "STARRED",
    "IMPORTANT",
    "UNREAD",
    "CHAT",
    "CATEGORY_PERSONAL",
    "CATEGORY_SOCIAL",
    "CATEGORY_FORUMS",
    "CATEGORY_UPDATES",
    "CATEGORY_PROMOTIONS",
];

/// Whether this Gmail label is a user topic the classifier may choose.
/// System labels (`INBOX`, `SENT`, `STARRED`, …) and Gmail's virtual names
/// (`CATEGORY_*`, `[Imap]/…`, `[Gmail]/…`, `[Google Mail]/…`) are excluded
/// even when they arrive typed as `user`.
pub fn is_classifiable_label(label: &Label) -> bool {
    if !label.label_type.eq_ignore_ascii_case("user") {
        return false;
    }
    let id = label.id.trim();
    if !id.is_empty()
        && EXCLUDED_CLASSIFICATION_IDS
            .iter()
            .any(|excluded| id.eq_ignore_ascii_case(excluded))
    {
        return false;
    }
    let name = label.name.trim();
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("category_")
        || lower.starts_with("[imap]/")
        || lower.starts_with("[gmail]/")
        || lower.starts_with("[google mail]/")
    {
        return false;
    }
    if lower == "chat" || lower == "draft" {
        return false;
    }
    !name.is_empty()
}

/// The subset of `labels` the classifier may be offered.
pub fn classifiable_labels(labels: &[Label]) -> Vec<Label> {
    labels
        .iter()
        .filter(|label| is_classifiable_label(label))
        .cloned()
        .collect()
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
    #[serde(default)]
    pub label_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(id: &str, name: &str, label_type: &str) -> Label {
        Label {
            id: id.into(),
            name: name.into(),
            label_type: label_type.into(),
            message_list_visibility: None,
            label_list_visibility: None,
        }
    }

    #[test]
    fn system_labels_are_not_classifiable() {
        for (id, name) in [
            ("INBOX", "INBOX"),
            ("SPAM", "SPAM"),
            ("TRASH", "TRASH"),
            ("DRAFT", "DRAFT"),
            ("SENT", "SENT"),
            ("STARRED", "STARRED"),
            ("IMPORTANT", "IMPORTANT"),
            ("UNREAD", "UNREAD"),
            ("CHAT", "CHAT"),
            ("CATEGORY_PROMOTIONS", "CATEGORY_PROMOTIONS"),
            ("CATEGORY_SOCIAL", "CATEGORY_SOCIAL"),
        ] {
            assert!(
                !is_classifiable_label(&label(id, name, "system")),
                "{id} should be excluded"
            );
            // Even if Gmail ever types one as user, it stays excluded.
            assert!(
                !is_classifiable_label(&label(id, name, "user")),
                "{id} as user should still be excluded"
            );
        }
    }

    #[test]
    fn virtual_names_are_not_classifiable() {
        assert!(!is_classifiable_label(&label(
            "Label_99",
            "[Imap]/Trash",
            "user"
        )));
        assert!(!is_classifiable_label(&label(
            "Label_99",
            "[Gmail]/Trash",
            "user"
        )));
        assert!(!is_classifiable_label(&label(
            "Label_99",
            "CATEGORY_FORUMS",
            "user"
        )));
    }

    #[test]
    fn user_topics_are_classifiable() {
        assert!(is_classifiable_label(&label("Label_1", "Finance", "user")));
        assert!(is_classifiable_label(&label(
            "Label_2",
            "Education/High",
            "user"
        )));
        assert_eq!(
            classifiable_labels(&[
                label("INBOX", "INBOX", "system"),
                label("Label_1", "Finance", "user"),
                label("Label_99", "[Imap]/Trash", "user"),
            ])
            .iter()
            .map(|l| l.name.clone())
            .collect::<Vec<_>>(),
            vec!["Finance".to_string()]
        );
    }
}
