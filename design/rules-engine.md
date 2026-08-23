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
    /// The outcomes the model picks between; empty asks a plain match-or-not question.
    pub choices: Vec<Action>,
    pub choose_from_all_labels: bool,
    /// Runs on every match, whatever the model chose.
    pub actions: Vec<Action>,
    pub priority: i32,
    pub enabled: bool,
    pub parent_id: Option<i64>,
    pub continue_after_match: bool,
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

    #[serde(rename = "remove_label")]
    RemoveLabel { value: String },

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

Rules run in priority order. Deterministic conditions are a cheap pre-filter.

- No prompt and no menu: the action recipe runs and claims the email, with no
  inference at all.
- Otherwise the model answers with a choice or `NO_MATCH`. `NO_MATCH` continues
  to the next rule; a choice claims the email and runs both the chosen action and
  the rule's recipe.
- A choice may be a label or an action, so one rule can say "file it under A, B,
  or bin it". The model picks by name and cannot create labels or invent
  operations outside the menu.
- Recoverable deviations (an unknown name, a selection with no menu, a bare
  action token) resolve and carry a non-fatal note.
- A reply that carries no readable decision for an email queues that email for a
  per-email re-ask and does not fall through in the meantime.

A match claims the email and stops evaluation. `continue_after_match` opts a rule
out of that: its actions run and the email still passes to lower-priority rules,
which is how a classifier can add a label and let a later rule archive it. An
unreadable reply never continues, opt-in or not: the queued re-ask owns the email
from there, and running lower rules first would race it.

Continuation does not re-fetch the message, so a later rule's label conditions do
not see labels applied earlier in the same pass; they take effect on the next run.

`engine::rules_after` is the single definition of "the next rule": everything
after the declining rule in priority order that is enabled and passes its own
deterministic conditions. The live pipeline, the retry queue, and the rule tester
all walk it, so the tester cannot disagree with production.

A chain that ends without any rule claiming the email records a single
`FALLTHROUGH_EXHAUSTED` history row. Condition-filtered rules are otherwise
invisible, which makes "fallthrough found nothing" indistinguishable from
"fallthrough never ran".

```rust
// crates/core/src/rules/engine.rs (simplified)
pub(crate) fn resolve_effective_actions(
    rule: &Rule,
    parsed: Option<ParsedAction>,
) -> Vec<ParsedAction> {
    match parsed {
        None => vec![],
        Some(ParsedAction::Apply) => rule.actions.iter().map(ParsedAction::from).collect(),
        // Destructive tokens run alone so they cannot drag configured labels along.
        Some(ParsedAction::Trash) => vec![ParsedAction::Trash],
        Some(ParsedAction::Spam) => vec![ParsedAction::Spam],
        Some(action) => {
            let mut actions = vec![action];
            actions.extend(
                rule.actions
                    .iter()
                    .map(ParsedAction::from)
                    .filter(|configured| !actions.contains(configured)),
            );
            actions
        }
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
        ParsedAction::Label(value) => (vec![resolve_existing_label_id(value, existing_labels)?], vec![]),
        ParsedAction::RemoveLabel(value) => (vec![], vec![resolve_existing_label_id(value, existing_labels)?]),
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

/// Labels are never created automatically: a rule may only act on labels the
/// user already has, so an unresolvable reference is an error.
fn resolve_existing_label_id(
    value: &str,
    existing_labels: &[Label],
) -> Result<String, RuleError> {
    find_label(value, existing_labels)
        .map(|label| label.id.clone())
        .ok_or_else(|| unknown_label(value))
}
```

`existing_labels` comes from the per-account label cache, so a label added in
Gmail resolves once that cache refreshes.

## Rule Priority and Evaluation

### Priority And Fallthrough

Rules are evaluated by priority (lower number = higher priority). The first
deterministic or LLM-confirmed match wins. A decision rule that returns
`NO_MATCH` is deliberately not a match, so lower-priority rules can inspect the
same email. This costs additional LLM calls only when broad rules decline an
email, so deterministic conditions should remain selective.

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

The polling service fetches emails matching the query, then evaluates rules against each one. A rule with no prompt and no menu runs its actions directly. Otherwise the model answers with a choice or `NO_MATCH`, and a match runs the chosen action plus the rule's recipe. If no rule matches, the email is skipped.

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

No menu, so the model answers `MATCH` or `NO_MATCH`; a match labels and
archives.

Expected LLM response: `1: MATCH | Weekly newsletter digest.`

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
  "prompt": "Does this email contain an invoice or payment request?",
  "choices": [{ "type": "label", "value": "Invoices" }],
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
  "prompt": "This is from my boss.",
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
  "prompt": "Does this look like spam or a scam? Answer NO_MATCH if unsure.",
  "choices": [{ "type": "spam" }],
  "actions": [],
  "priority": 50
}
```
