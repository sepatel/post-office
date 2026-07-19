use crate::db::rules::RuleRepository;
use crate::gmail::models::Message;
use crate::llm::{LlmClient, ProcessRequest};
use crate::rules::prompts::RULE_SYSTEM_PROMPT;
use super::matcher;
use super::models::Rule;
use super::response_parser::{extract_reasoning, parse_llm_response, ParsedAction};

pub struct RuleEngine<'a> {
    rule_repo: &'a RuleRepository<'a>,
}

impl<'a> RuleEngine<'a> {
    pub fn new(rule_repo: &'a RuleRepository<'a>) -> Self {
        Self { rule_repo }
    }

    /// Sync: load enabled rules and return the highest-priority one whose
    /// conditions all match (None if nothing matches).
    pub fn find_matching_rule(
        &self,
        email: &Message,
        current_labels: &[String],
    ) -> Option<Rule> {
        let rules = self.rule_repo.get_enabled_rules().ok()?;
        rules
            .into_iter()
            .filter(|r| {
                r.conditions
                    .iter()
                    .all(|c| matcher::evaluate(c, email, current_labels))
            })
            .min_by_key(|r| r.priority)
    }
}

/// Resolve a single rule against an email without persisting anything.
/// Returns `None` when the rule's conditions don't match; otherwise the
/// actions that *would* be taken (structured actions, or the LLM's verdict)
/// plus the raw LLM reply (empty for the structured-action path).
///
/// Takes `&LlmClient` directly so callers don't need a `RuleRepository` — this
/// is what lets the test/apply commands run without blocking the async runtime.
pub async fn resolve_rule(
    llm: &LlmClient,
    rule: &Rule,
    email: &Message,
    memories: &[String],
) -> Result<Option<Resolved>, RuleError> {
    let matched = rule
        .conditions
        .iter()
        .all(|c| matcher::evaluate(c, email, &email.label_ids));

    if !matched {
        return Ok(None);
    }

    let result = execute_rule(llm, rule, email, memories).await?;
    Ok(Some(result))
}

/// Dry-run variant of `resolve_rule` that produces a frontend-friendly result.
pub async fn test_rule(
    llm: &LlmClient,
    rule: &Rule,
    email: &Message,
    memories: &[String],
) -> Result<TestResult, RuleError> {
    match resolve_rule(llm, rule, email, memories).await? {
        None => Ok(TestResult {
            matched: false,
            actions: vec![],
            llm_response: String::new(),
            reasoning: String::new(),
        }),
        Some(resolved) => Ok(TestResult {
            matched: true,
            actions: resolved.actions.iter().map(display_action).collect(),
            llm_response: resolved.llm_response,
            reasoning: resolved.reasoning,
        }),
    }
}

async fn execute_rule(
    llm: &LlmClient,
    rule: &Rule,
    email: &Message,
    memories: &[String],
) -> Result<Resolved, RuleError> {
    // No prompt: structured actions (if any) run deterministically; otherwise
    // nothing happens. The LLM is only consulted when a rule has a prompt.
    if rule.prompt.trim().is_empty() {
        if rule.actions.is_empty() {
            return Ok(Resolved {
                actions: vec![],
                llm_response: String::new(),
                reasoning: String::new(),
            });
        }
        let actions: Vec<ParsedAction> = rule.actions.iter().map(ParsedAction::from).collect();
        return Ok(Resolved {
            actions,
            llm_response: String::new(),
            reasoning: String::new(),
        });
    }

    // Prompt present: the LLM decides APPLY/SKIP (and may name a label). The
    // prompt is a fuzzy gate; structured actions, when present, are the
    // deterministic outcome of a non-SKIP verdict.
    let user_prompt = build_user_prompt(&rule.prompt, email, memories);

    let llm_response = llm
        .process(ProcessRequest {
            system_prompt: Some(RULE_SYSTEM_PROMPT.to_string()),
            user_prompt,
            model: None,
            temperature: None,
            max_tokens: None,
        })
        .await?;

    let parsed = parse_llm_response(&llm_response.content);

    let actions = match parsed {
        None => vec![],
        Some(action) => {
            if !rule.actions.is_empty() {
                rule.actions.iter().map(ParsedAction::from).collect()
            } else {
                vec![action]
            }
        }
    };

    Ok(Resolved {
        actions,
        llm_response: llm_response.content.clone(),
        reasoning: extract_reasoning(&llm_response.content),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),

    #[error(transparent)]
    Llm(#[from] crate::llm::LlmError),
}

/// Outcome of resolving a rule against one email (used by test/apply paths).
pub struct Resolved {
    pub actions: Vec<ParsedAction>,
    pub llm_response: String,
    pub reasoning: String,
}

/// Frontend-friendly result of a dry-run test against a single email.
#[derive(Debug, serde::Serialize)]
pub struct TestResult {
    pub matched: bool,
    pub actions: Vec<ActionDisplay>,
    pub llm_response: String,
    pub reasoning: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ActionDisplay {
    pub kind: String,
    pub detail: Option<String>,
    pub display: String,
}

pub fn display_action(action: &ParsedAction) -> ActionDisplay {
    match action {
        ParsedAction::Label(name) => ActionDisplay {
            kind: "label".into(),
            detail: Some(name.clone()),
            display: format!("Add label \"{}\"", name),
        },
        ParsedAction::Archive => ActionDisplay {
            kind: "archive".into(),
            detail: None,
            display: "Archive".into(),
        },
        ParsedAction::Trash => ActionDisplay {
            kind: "trash".into(),
            detail: None,
            display: "Move to Trash".into(),
        },
        ParsedAction::Spam => ActionDisplay {
            kind: "spam".into(),
            detail: None,
            display: "Mark as Spam".into(),
        },
        ParsedAction::MarkRead => ActionDisplay {
            kind: "mark_read".into(),
            detail: None,
            display: "Mark as read".into(),
        },
        ParsedAction::MarkUnread => ActionDisplay {
            kind: "mark_unread".into(),
            detail: None,
            display: "Mark as unread".into(),
        },
        ParsedAction::Star => ActionDisplay {
            kind: "star".into(),
            detail: None,
            display: "Star".into(),
        },
        ParsedAction::Apply => ActionDisplay {
            kind: "apply".into(),
            detail: None,
            display: "Apply rule actions".into(),
        },
    }
}

/// Headers and plain-text body of an email, used both by the single-email prompt
/// and by the batch evaluator. Kept as a shared primitive so the two paths can't
/// drift in how an email is rendered to the model.
pub(crate) fn email_parts(email: &Message) -> (String, String) {
    let headers = email
        .payload
        .as_ref()
        .map(|p| {
            p.headers
                .iter()
                .map(|header| format!("{}: {}", header.name, header.value))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    let body = extract_plain_text(email);

    (headers, body)
}

fn build_user_prompt(rule_prompt: &str, email: &Message, memories: &[String]) -> String {
    let (headers, body) = email_parts(email);

    let memory_block = if memories.is_empty() {
        String::new()
    } else {
        format!(
            "\n\n--- Learned Memory ---\n{}\n(These are exceptions and notes you must respect when deciding.)",
            memories.join("\n")
        )
    };

    format!(
        "--- Email Headers ---\n\
         {headers}\n\n\
         --- Email Body ---\n\
         {body}\n\n\
         --- Rule Instruction ---\n\
         {rule_prompt}{memory_block}"
    )
}

fn extract_plain_text(email: &Message) -> String {
    use crate::gmail::models::MessagePayload;

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
            .map(decode_base64url)
    };

    let find_html = |parts: &[MessagePayload]| -> Option<String> {
        parts
            .iter()
            .find(|p| p.mime_type == "text/html")
            .and_then(|p| p.body.as_ref())
            .and_then(|b| b.data.as_deref())
            .map(|data| strip_html(&decode_base64url(data)))
    };

    payload
        .body
        .as_ref()
        .and_then(|b| b.data.as_deref())
        .filter(|_| payload.mime_type == "text/plain")
        .map(decode_base64url)
        .or_else(|| payload.parts.as_deref().and_then(&find_plain))
        .or_else(|| payload.parts.as_deref().and_then(&find_html))
        .unwrap_or_else(|| email.snippet.clone())
}

fn decode_base64url(data: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE
        .decode(data)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn strip_html(html: &str) -> String {
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
