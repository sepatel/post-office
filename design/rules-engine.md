# Rules Engine

## Overview

The rules engine evaluates email conditions against user-defined rules and executes corresponding actions via the LLM and Gmail API.

## Rule Structure

```rust
// crates/core/src/rules/models.rs

pub struct Rule {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub conditions: Vec<Condition>,
    pub prompt: String,
    pub actions: Vec<Action>,
    pub priority: i32,
    pub enabled: bool,
    pub parent_id: Option<i64>,
}
```

## Conditions

Conditions are composable and support logical operators.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Condition {
    #[serde(rename = "from")]
    From { operator: Operator, value: String },

    #[serde(rename = "to")]
    To { operator: Operator, value: String },

    #[serde(rename = "subject")]
    Subject { operator: Operator, value: String },

    #[serde(rename = "body")]
    Body { operator: Operator, value: String },

    #[serde(rename = "has_attachment")]
    HasAttachment { value: bool },

    #[serde(rename = "is_unread")]
    IsUnread { value: bool },

    #[serde(rename = "label")]
    Label { operator: Operator, value: String },

    #[serde(rename = "date_after")]
    DateAfter { value: String },

    #[serde(rename = "date_before")]
    DateBefore { value: String },

    #[serde(rename = "and")]
    And { conditions: Vec<Condition> },

    #[serde(rename = "or")]
    Or { conditions: Vec<Condition> },

    #[serde(rename = "not")]
    Not { condition: Box<Condition> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Contains,
    Equals,
    Regex,
    NotContains,
}
```

## Actions

Actions map directly to Gmail API label operations.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Action {
    #[serde(rename = "label")]
    Label { value: String },

    #[serde(rename = "archive")]
    Archive,

    #[serde(rename = "trash")]
    Trash,

    #[serde(rename = "spam")]
    Spam,

    #[serde(rename = "mark_read")]
    MarkRead,

    #[serde(rename = "mark_unread")]
    MarkUnread,

    #[serde(rename = "star")]
    Star,
}
```

## Condition Evaluation

```rust
// crates/core/src/rules/matcher.rs

use crate::gmail::models::{Message, MessagePayload};
use chrono::NaiveDate;

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
        Condition::IsUnread { value } => {
            current_labels.iter().any(|l| l == "UNREAD") == *value
        }
        Condition::Label { operator, value } => {
            current_labels.iter().any(|l| evaluate_string_op(l, operator, value))
        }
        Condition::DateAfter { value } => NaiveDate::parse_from_str(value, "%Y/%m/%d")
            .ok()
            .and_then(|date| parse_email_date(email).map(|d| d >= date))
            .unwrap_or(false),
        Condition::DateBefore { value } => NaiveDate::parse_from_str(value, "%Y/%m/%d")
            .ok()
            .and_then(|date| parse_email_date(email).map(|d| d <= date))
            .unwrap_or(false),
        Condition::And { conditions } => {
            conditions.iter().all(|c| evaluate(c, email, current_labels))
        }
        Condition::Or { conditions } => {
            conditions.iter().any(|c| evaluate(c, email, current_labels))
        }
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
        .and_then(|p| p.headers.as_ref())
        .and_then(|headers| headers.iter().find(|h| h.name == name))
        .map(|h| h.value.clone())
        .unwrap_or_default()
}

fn email_has_attachment(email: &Message) -> bool {
    email
        .payload
        .as_ref()
        .is_some_and(has_attachment_recursive)
}

fn has_attachment_recursive(payload: &MessagePayload) -> bool {
    payload
        .filename
        .as_ref()
        .is_some_and(|f| !f.is_empty())
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
```

## Rule Evaluation Flow

Rules run in first-match-wins order (lowest `priority` value). For the matched
rule, execution depends on whether a prompt exists:

- No prompt: execute `rule.actions` directly (deterministic, no LLM call)
- Prompt present: parse first token from LLM response and resolve hybridly:
  - `APPLY` => execute `rule.actions`
  - `SKIP` or invalid => no action
  - explicit token (`ARCHIVE`, `TRASH`, `SPAM`, `MARK_READ`, `MARK_UNREAD`, `STAR`, `LABEL: <name>`) => execute token directly

```rust
// crates/core/src/rules/engine.rs (simplified)
pub(crate) fn resolve_effective_actions(
    rule: &Rule,
    parsed: Option<ParsedAction>,
) -> Vec<ParsedAction> {
    match parsed {
        None => vec![],
        Some(ParsedAction::Apply) => rule.actions.iter().map(ParsedAction::from).collect(),
        Some(action) => vec![action],
    }
}
```

## Action Execution

```rust
// crates/core/src/rules/actions.rs

use super::response_parser::ParsedAction;
use crate::gmail::models::Label;
use crate::gmail::GmailClient;

pub async fn execute_action(
    gmail: &mut GmailClient,
    email_id: &str,
    action: &ParsedAction,
    existing_labels: &[Label],
) -> Result<(), RuleError> {
    let (add, remove) = match action {
        ParsedAction::Label(name) => {
            let label = get_or_create_label(gmail, name, existing_labels).await?;
            (vec![label.id], vec![])
        }
        ParsedAction::Archive => (vec![], vec!["INBOX".into()]),
        ParsedAction::Trash => (vec!["TRASH".into()], vec!["INBOX".into()]),
        ParsedAction::Spam => (vec!["SPAM".into()], vec!["INBOX".into()]),
        ParsedAction::MarkRead => (vec![], vec!["UNREAD".into()]),
        ParsedAction::MarkUnread => (vec!["UNREAD".into()], vec![]),
        ParsedAction::Star => (vec!["STARRED".into()], vec![]),
        ParsedAction::Apply => (vec![], vec![]),
    };

    if add.is_empty() && remove.is_empty() {
        return Ok(());
    }

    let add_refs: Vec<&str> = add.iter().map(String::as_str).collect();
    let remove_refs: Vec<&str> = remove.iter().map(String::as_str).collect();
    gmail.modify_labels(email_id, &add_refs, &remove_refs).await?;

    Ok(())
}

async fn get_or_create_label(
    gmail: &mut GmailClient,
    name: &str,
    existing_labels: &[Label],
) -> Result<crate::gmail::models::Label, RuleError> {
    if let Some(existing) = existing_labels.iter().find(|l| l.name == name) {
        return Ok(existing.clone());
    }

    gmail.create_label(name).await
}
```

## Rule Priority and Evaluation

### First-Match Mode (Only Mode)

Rules are evaluated by priority (lower number = higher priority). The first matching rule wins. This is the only evaluation mode — simpler, more predictable, and cheaper (fewer LLM calls).

```rust
let matching_rule = rules.iter()
    .filter(|r| r.conditions.iter().all(|c| ConditionEvaluator::evaluate(c, email, current_labels)))
    .min_by_key(|r| r.priority);

match matching_rule {
    Some(rule) => execute_rule(rule, email).await,
    None => skip(email),  // No rules matched
}
```

## Processing Scope

The Gmail query used to fetch emails is configurable. Stored in the config table:

```json
{
  "polling.query": "is:unread",
  "polling.interval_minutes": "5",
  "polling.max_per_cycle": "100",
  "polling.enabled": "true"
}
```

### Common Queries

| Use Case | Gmail Query |
|----------|-------------|
| Default (unread only) | `is:unread` |
| Inbox only | `in:inbox is:unread` |
| Last 7 days | `newer_than:7d is:unread` |
| Specific label | `label:Newsletters is:unread` |
| Exclude categories | `in:inbox -category:promotions -category:social is:unread` |

The polling service fetches emails matching the query, then evaluates rules against each one. If a rule matches and has no prompt, configured actions run directly. If it has a prompt, the LLM token is resolved with hybrid semantics (`APPLY` runs configured actions, explicit tokens override per email, `SKIP`/invalid does nothing). If no rule matches, the email is skipped.

## Example Rules

### Rule 1: Newsletter Auto-Archive

```json
{
  "name": "Archive Newsletters",
  "conditions": [
    {
      "type": "from",
      "operator": "contains",
      "value": "newsletter@"
    }
  ],
  "prompt": "This is a newsletter email.",
  "actions": [
    { "type": "label", "value": "Newsletters" },
    { "type": "archive" }
  ],
  "priority": 10
}
```

The LLM will see:
```
--- Email Headers ---
From: newsletter@example.com
Subject: Weekly Update

--- Email Body ---
This week's top stories...

--- Rule Instruction ---
This is a newsletter email.

--- Response Format ---
Respond with EXACTLY two lines:
1) Action token: ARCHIVE, TRASH, SPAM, MARK_READ, MARK_UNREAD, STAR, APPLY, SKIP, or LABEL: <name>
2) One-sentence imperative explanation
```

Expected LLM response token: `APPLY`

### Rule 2: Invoice Detection

```json
{
  "name": "Flag Invoices",
  "conditions": [
    {
      "type": "or",
      "conditions": [
        { "type": "subject", "operator": "contains", "value": "invoice" },
        { "type": "subject", "operator": "regex", "value": "(?i)payment due" }
      ]
    }
  ],
  "prompt": "Does this email contain an invoice or payment request? If yes, respond with LABEL: Invoices. If no, respond with SKIP.",
  "actions": [],
  "priority": 20
}
```

### Rule 3: Important Sender

```json
{
  "name": "Boss Emails",
  "conditions": [
    { "type": "from", "operator": "equals", "value": "boss@company.com" }
  ],
  "prompt": "This is from my boss. Respond with STAR.",
  "actions": [
    { "type": "star" }
  ],
  "priority": 5
}
```

### Rule 4: Spam-like Pattern

```json
{
  "name": "Suspicious Marketing",
  "conditions": [
    {
      "type": "and",
      "conditions": [
        { "type": "subject", "operator": "regex", "value": "(?i)(act now|limited time|free|winner)" },
        { "type": "from", "operator": "not_contains", "value": "@known-trusted.com" }
      ]
    }
  ],
  "prompt": "Does this look like spam or a scam? If yes, respond with SPAM. If unsure, respond with SKIP.",
  "actions": [],
  "priority": 50
}
```
