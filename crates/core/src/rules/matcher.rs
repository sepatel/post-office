use chrono::NaiveDate;

use super::models::{Condition, Operator};
use crate::gmail::models::{Message, MessagePayload};

pub fn evaluate(condition: &Condition, email: &Message, current_labels: &[String]) -> bool {
    match condition {
        Condition::From { operator, value } => {
            let sender = extract_header(email, "From");
            evaluate_string_op(&sender, operator, value)
        }
        Condition::To { operator, value } => {
            let to = extract_header(email, "To");
            evaluate_string_op(&to, operator, value)
        }
        Condition::Subject { operator, value } => {
            let subject = extract_header(email, "Subject");
            evaluate_string_op(&subject, operator, value)
        }
        Condition::Body { operator, value } => {
            let body = extract_plain_text(email);
            evaluate_string_op(&body, operator, value)
        }
        Condition::HasAttachment { value } => email_has_attachment(email) == *value,
        Condition::IsUnread { value } => current_labels.iter().any(|l| l == "UNREAD") == *value,
        Condition::Label { operator, value } => current_labels
            .iter()
            .any(|l| evaluate_string_op(l, operator, value)),
        Condition::DateAfter { value } => NaiveDate::parse_from_str(value, "%Y/%m/%d")
            .ok()
            .and_then(|date| parse_email_date(email).map(|d| d >= date))
            .unwrap_or(false),
        Condition::DateBefore { value } => NaiveDate::parse_from_str(value, "%Y/%m/%d")
            .ok()
            .and_then(|date| parse_email_date(email).map(|d| d <= date))
            .unwrap_or(false),
        Condition::And { conditions } => conditions
            .iter()
            .all(|c| evaluate(c, email, current_labels)),
        Condition::Or { conditions } => conditions
            .iter()
            .any(|c| evaluate(c, email, current_labels)),
        Condition::Not { condition } => !evaluate(condition, email, current_labels),
    }
}

fn evaluate_string_op(haystack: &str, op: &Operator, needle: &str) -> bool {
    match op {
        Operator::Contains => haystack.to_lowercase().contains(&needle.to_lowercase()),
        Operator::Equals => haystack.to_lowercase() == needle.to_lowercase(),
        Operator::Regex => regex::Regex::new(needle)
            .map(|re| re.is_match(haystack))
            .unwrap_or(false),
        Operator::NotContains => !haystack.to_lowercase().contains(&needle.to_lowercase()),
    }
}

fn extract_header(email: &Message, name: &str) -> String {
    email
        .payload
        .as_ref()
        .and_then(|p| {
            p.headers
                .iter()
                .find(|h| h.name == name)
                .map(|h| h.value.clone())
        })
        .unwrap_or_default()
}

fn email_has_attachment(email: &Message) -> bool {
    email.payload.as_ref().is_some_and(has_attachment_recursive)
}

fn has_attachment_recursive(payload: &MessagePayload) -> bool {
    payload.filename.as_ref().is_some_and(|f| !f.is_empty())
        || payload
            .parts
            .as_ref()
            .is_some_and(|parts| parts.iter().any(has_attachment_recursive))
}

fn parse_email_date(email: &Message) -> Option<NaiveDate> {
    let date_str = extract_header(email, "Date");
    NaiveDate::parse_from_str(&date_str, "%a, %d %b %Y")
        .or_else(|_| NaiveDate::parse_from_str(&date_str, "%d %b %Y"))
        .ok()
}

fn extract_plain_text(email: &Message) -> String {
    let payload = match &email.payload {
        Some(p) => p,
        None => return email.snippet.clone(),
    };

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
