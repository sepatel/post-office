use chrono::NaiveDate;

use super::models::{Condition, Operator};
use crate::rules::email_view::EmailView;

pub fn evaluate(condition: &Condition, view: &EmailView) -> bool {
    match condition {
        Condition::From { operator, value } => {
            evaluate_string_op(view.header("From").unwrap_or_default(), operator, value)
        }
        Condition::To { operator, value } => {
            evaluate_string_op(view.header("To").unwrap_or_default(), operator, value)
        }
        Condition::Subject { operator, value } => {
            evaluate_string_op(view.header("Subject").unwrap_or_default(), operator, value)
        }
        Condition::Body { operator, value } => evaluate_string_op(&view.body, operator, value),
        Condition::HasAttachment { value } => view.has_attachments == *value,
        Condition::IsUnread { value } => view.label_ids.iter().any(|l| l == "UNREAD") == *value,
        Condition::Label { operator, value } => view
            .label_ids
            .iter()
            .any(|l| evaluate_string_op(l, operator, value)),
        Condition::DateAfter { value } => NaiveDate::parse_from_str(value, "%Y/%m/%d")
            .ok()
            .and_then(|date| parse_email_date(view).map(|d| d >= date))
            .unwrap_or(false),
        Condition::DateBefore { value } => NaiveDate::parse_from_str(value, "%Y/%m/%d")
            .ok()
            .and_then(|date| parse_email_date(view).map(|d| d <= date))
            .unwrap_or(false),
        Condition::And { conditions } => conditions.iter().all(|c| evaluate(c, view)),
        Condition::Or { conditions } => conditions.iter().any(|c| evaluate(c, view)),
        Condition::Not { condition } => !evaluate(condition, view),
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

fn parse_email_date(view: &EmailView) -> Option<NaiveDate> {
    let date_str = view.header("Date")?;
    NaiveDate::parse_from_str(date_str, "%a, %d %b %Y")
        .or_else(|_| NaiveDate::parse_from_str(date_str, "%d %b %Y"))
        .ok()
}
