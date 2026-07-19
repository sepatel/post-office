use std::collections::HashMap;
use std::str::FromStr;
use std::sync::LazyLock;

use regex::Regex;

use crate::gmail::models::Message;
use crate::llm::{LlmClient, ProcessRequest};
use crate::rules::engine::{
    display_action, email_parts, resolve_effective_actions, ActionDisplay, RuleError,
};
use crate::rules::matcher;
use crate::rules::models::Rule;
use crate::rules::prompts::BATCH_SYSTEM_PROMPT;
use crate::rules::response_parser::ParsedAction;

/// Emails per LLM call. Kept small so local models (Ollama/llama3) stay within a
/// context they classify accurately; one call still covers many emails.
const BATCH_SIZE: usize = 10;

/// Proposed outcome for a single email after bulk evaluation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BulkVerdict {
    pub email_id: String,
    /// True when the rule's conditions matched (the email was actually considered).
    pub matched: bool,
    pub actions: Vec<ActionDisplay>,
    /// Raw LLM reply for the batch this email belonged to (empty for rules that
    /// resolve locally without calling the model).
    pub llm_response: String,
}

/// Evaluate a rule against many emails in bounded batches. Emails whose
/// conditions don't match are reported with `matched: false` and no LLM call.
/// Structured-action rules resolve locally; only prompt-based rules hit the LLM,
/// one call per `BATCH_SIZE` matched emails.
pub async fn bulk_evaluate(
    llm: &LlmClient,
    rule: &Rule,
    emails: &[Message],
) -> Result<Vec<BulkVerdict>, RuleError> {
    let mut verdicts: Vec<BulkVerdict> = emails
        .iter()
        .map(|e| BulkVerdict {
            email_id: e.id.clone(),
            matched: false,
            actions: vec![],
            llm_response: String::new(),
        })
        .collect();

    let matched: Vec<usize> = emails
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            rule.conditions
                .iter()
                .all(|c| matcher::evaluate(c, e, &e.label_ids))
        })
        .map(|(i, _)| i)
        .collect();

    if matched.is_empty() {
        return Ok(verdicts);
    }

    for &i in &matched {
        verdicts[i].matched = true;
    }

    // No prompt: structured actions resolve deterministically, no LLM call.
    if rule.prompt.trim().is_empty() {
        for &i in &matched {
            verdicts[i].actions = rule
                .actions
                .iter()
                .map(ParsedAction::from)
                .map(|a| display_action(&a))
                .collect();
        }
        return Ok(verdicts);
    }

    let matched_emails: Vec<&Message> = matched.iter().map(|&i| &emails[i]).collect();
    let mut chunk_start = 0;
    for chunk in matched_emails.chunks(BATCH_SIZE) {
        let user_prompt = build_batch_prompt(rule, chunk);
        let response = llm
            .process(ProcessRequest {
                system_prompt: Some(BATCH_SYSTEM_PROMPT.to_string()),
                user_prompt,
                model: None,
                temperature: None,
                max_tokens: None,
            })
            .await
            .map_err(RuleError::Llm)?;

        let parsed = parse_batch(&response.content, chunk.len());
        for (offset, action_opt) in parsed.into_iter().enumerate() {
            let global_idx = matched[chunk_start + offset];
            verdicts[global_idx].actions = resolve_effective_actions(rule, action_opt)
                .into_iter()
                .map(|a| display_action(&a))
                .collect();
            verdicts[global_idx].llm_response = response.content.clone();
        }
        chunk_start += chunk.len();
    }

    Ok(verdicts)
}

fn build_batch_prompt(rule: &Rule, emails: &[&Message]) -> String {
    let mut s = String::new();
    s.push_str("RULE INSTRUCTION:\n");
    s.push_str(rule.prompt.trim());
    s.push_str("\n\nEMAILS:\n");
    for (i, email) in emails.iter().enumerate() {
        let (headers, body) = email_parts(email);
        s.push_str(&format!("--- EMAIL {} ---\n{headers}\n\n{body}\n", i + 1));
    }
    s.push_str(&format!(
        "\nRespond with one `N: <TOKEN>` line per email, in order (1 to {}).\n",
        emails.len()
    ));
    s
}

static BATCH_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\d+)\s*:\s*(.+?)\s*$").unwrap());

/// Parse `N: <TOKEN>` lines into per-email actions. Unknown indices, garbled
/// lines, and SKIP all become `None` (no action) — the same safe default the
/// single-email path uses for an unparsable or SKIP response.
fn parse_batch(response: &str, count: usize) -> Vec<Option<ParsedAction>> {
    let mut map: HashMap<usize, ParsedAction> = HashMap::new();
    for line in response.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(caps) = BATCH_LINE_RE.captures(line) else {
            continue;
        };
        let Ok(idx) = caps.get(1).unwrap().as_str().parse::<usize>() else {
            continue;
        };
        if idx == 0 {
            continue;
        }
        if let Ok(action) = ParsedAction::from_str(caps.get(2).unwrap().as_str().trim()) {
            map.insert(idx, action);
        }
    }
    (1..=count).map(|i| map.get(&i).cloned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_indexed_token_lines() {
        let parsed = parse_batch("1: ARCHIVE\n2: LABEL: Invoices\n3: SKIP\n", 3);
        assert_eq!(parsed[0], Some(ParsedAction::Archive));
        assert_eq!(parsed[1], Some(ParsedAction::Label("Invoices".into())));
        assert_eq!(parsed[2], None);
    }

    #[test]
    fn missing_index_is_skip() {
        let parsed = parse_batch("1: TRASH\n3: STAR", 3);
        assert_eq!(parsed[0], Some(ParsedAction::Trash));
        assert_eq!(parsed[1], None);
        assert_eq!(parsed[2], Some(ParsedAction::Star));
    }

    #[test]
    fn garbled_line_is_ignored() {
        let parsed = parse_batch("1: ARCHIVE\nbanana\n2: SPAM", 2);
        assert_eq!(parsed[0], Some(ParsedAction::Archive));
        assert_eq!(parsed[1], Some(ParsedAction::Spam));
    }
}
