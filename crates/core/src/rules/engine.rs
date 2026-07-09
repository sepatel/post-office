use crate::db::rules::RuleRepository;
use crate::gmail::models::Message;
use crate::llm::LlmClient;
use super::matcher;
use super::models::Rule;
use super::response_parser::{parse_llm_response, ParsedAction};

pub struct RuleEngine<'a> {
    rule_repo: &'a RuleRepository<'a>,
    llm: &'a LlmClient,
}

impl<'a> RuleEngine<'a> {
    pub fn new(rule_repo: &'a RuleRepository<'a>, llm: &'a LlmClient) -> Self {
        Self { rule_repo, llm }
    }

    pub async fn process_email(
        &self,
        email: &Message,
        current_labels: &[String],
    ) -> Result<ProcessingResult, RuleError> {
        let rules = self.rule_repo.get_enabled_rules()?;

        let matching_rule = rules
            .iter()
            .filter(|r| r.conditions.iter().all(|c| matcher::evaluate(c, email, current_labels)))
            .min_by_key(|r| r.priority);

        match matching_rule {
            Some(rule) => {
                let result = self.execute_rule(rule, email).await?;
                Ok(ProcessingResult::RuleMatched {
                    rule_id: rule.id,
                    rule_name: rule.name.clone(),
                    action: result.action_taken,
                    llm_response: result.llm_response,
                    duration_ms: result.duration_ms,
                })
            }
            None => Ok(ProcessingResult::NoMatch),
        }
    }

    async fn execute_rule(&self, rule: &Rule, email: &Message) -> Result<ExecutionResult, RuleError> {
        let user_prompt = build_user_prompt(&rule.prompt, email);

        let llm_response = self
            .llm
            .process(crate::llm::ProcessRequest::new(user_prompt))
            .await?;

        let parsed_action = parse_llm_response(&llm_response.content);

        Ok(ExecutionResult {
            action_taken: parsed_action,
            llm_response: llm_response.content,
            duration_ms: llm_response.duration_ms,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),

    #[error(transparent)]
    Llm(#[from] crate::llm::LlmError),
}

pub enum ProcessingResult {
    RuleMatched {
        rule_id: i64,
        rule_name: String,
        action: Option<ParsedAction>,
        llm_response: String,
        duration_ms: u64,
    },
    NoMatch,
}

struct ExecutionResult {
    action_taken: Option<ParsedAction>,
    llm_response: String,
    duration_ms: u64,
}

fn build_user_prompt(rule_prompt: &str, email: &Message) -> String {
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

    format!(
        "--- Email Headers ---\n\
         {headers}\n\n\
         --- Email Body ---\n\
         {body}\n\n\
         --- Rule Instruction ---\n\
         {rule_prompt}"
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
