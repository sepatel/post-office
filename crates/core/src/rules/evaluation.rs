use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

use regex::Regex;

use crate::gmail::models::Message;
use crate::llm::{InferenceRouter, ProcessRequest};
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
    /// This email's indexed LLM reply, including its explanation (empty for
    /// rules that resolve locally without calling the model).
    pub llm_response: String,
}

#[derive(Debug, Clone)]
pub struct BulkResolvedEmail {
    pub actions: Vec<ParsedAction>,
    pub llm_response: String,
    pub llm_model: Option<String>,
    pub llm_provider: Option<String>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub llm_duration_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct BulkRuleResolution {
    pub resolved: Vec<BulkResolvedEmail>,
    pub stopped: bool,
}

pub async fn bulk_resolve_for_rule(
    llm: &InferenceRouter,
    rule: &Rule,
    emails: &[Message],
    cancel_requested: Option<&AtomicBool>,
) -> Result<BulkRuleResolution, RuleError> {
    if emails.is_empty() {
        return Ok(BulkRuleResolution {
            resolved: vec![],
            stopped: false,
        });
    }

    if rule.prompt.trim().is_empty() {
        let actions: Vec<ParsedAction> = rule.actions.iter().map(ParsedAction::from).collect();
        return Ok(BulkRuleResolution {
            resolved: emails
                .iter()
                .map(|_| BulkResolvedEmail {
                    actions: actions.clone(),
                    llm_response: String::new(),
                    llm_model: None,
                    llm_provider: None,
                    prompt_tokens: None,
                    completion_tokens: None,
                    total_tokens: None,
                    llm_duration_ms: None,
                })
                .collect(),
            stopped: false,
        });
    }

    let mut resolved = Vec::with_capacity(emails.len());

    for chunk in emails.chunks(BATCH_SIZE) {
        if cancel_requested
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
        {
            return Ok(BulkRuleResolution {
                resolved,
                stopped: true,
            });
        }

        let refs: Vec<&Message> = chunk.iter().collect();
        let user_prompt = build_batch_prompt(rule, &refs);
        let response = llm
            .process(
                &rule.inference_policy,
                ProcessRequest {
                    system_prompt: Some(BATCH_SYSTEM_PROMPT.to_string()),
                    user_prompt,
                    model: None,
                    temperature: None,
                    max_tokens: None,
                },
            )
            .await
            .map_err(RuleError::Llm)?;

        let parsed = parse_batch(&response.content, chunk.len());
        let prompt_tokens = split_even(response.prompt_tokens, chunk.len());
        let completion_tokens = split_even(response.completion_tokens, chunk.len());
        let total_tokens = split_even(response.tokens_used, chunk.len());
        let duration_ms = split_even_u64(Some(response.duration_ms), chunk.len());

        for (offset, decision) in parsed.into_iter().enumerate() {
            resolved.push(BulkResolvedEmail {
                actions: resolve_effective_actions(rule, decision.action),
                llm_response: decision.llm_response,
                llm_model: Some(response.model.clone()),
                llm_provider: response.provider_id.clone(),
                prompt_tokens: prompt_tokens[offset],
                completion_tokens: completion_tokens[offset],
                total_tokens: total_tokens[offset],
                llm_duration_ms: duration_ms[offset],
            });
        }
    }

    Ok(BulkRuleResolution {
        resolved,
        stopped: false,
    })
}

/// Evaluate a rule against many emails in bounded batches. Emails whose
/// conditions don't match are reported with `matched: false` and no LLM call.
/// Structured-action rules resolve locally; only prompt-based rules hit the LLM,
/// one call per `BATCH_SIZE` matched emails.
pub async fn bulk_evaluate(
    llm: &InferenceRouter,
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
            .process(
                &rule.inference_policy,
                ProcessRequest {
                    system_prompt: Some(BATCH_SYSTEM_PROMPT.to_string()),
                    user_prompt,
                    model: None,
                    temperature: None,
                    max_tokens: None,
                },
            )
            .await
            .map_err(RuleError::Llm)?;

        let parsed = parse_batch(&response.content, chunk.len());
        for (offset, decision) in parsed.into_iter().enumerate() {
            let global_idx = matched[chunk_start + offset];
            verdicts[global_idx].actions = resolve_effective_actions(rule, decision.action)
                .into_iter()
                .map(|a| display_action(&a))
                .collect();
            verdicts[global_idx].llm_response = decision.llm_response;
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
        "\nRespond with one `N: <TOKEN> | <brief explanation>` line per email, in order (1 to {}).\n",
        emails.len()
    ));
    s
}

fn split_even(total: Option<u32>, parts: usize) -> Vec<Option<u32>> {
    let Some(total) = total else {
        return vec![None; parts];
    };
    if parts == 0 {
        return vec![];
    }
    let base = total / parts as u32;
    let remainder = total % parts as u32;
    (0..parts)
        .map(|idx| Some(base + u32::from((idx as u32) < remainder)))
        .collect()
}

fn split_even_u64(total: Option<u64>, parts: usize) -> Vec<Option<u64>> {
    let Some(total) = total else {
        return vec![None; parts];
    };
    if parts == 0 {
        return vec![];
    }
    let base = total / parts as u64;
    let remainder = total % parts as u64;
    (0..parts)
        .map(|idx| Some(base + u64::from((idx as u64) < remainder)))
        .collect()
}

static BATCH_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\d+)\s*:\s*(.+?)\s*$").unwrap());

#[derive(Debug, Clone, Default)]
struct BatchDecision {
    action: Option<ParsedAction>,
    llm_response: String,
}

/// Parse `N: <TOKEN> | <explanation>` lines into per-email decisions. The
/// explanation is retained with its email so history can show why it matched.
fn parse_batch(response: &str, count: usize) -> Vec<BatchDecision> {
    let mut map: HashMap<usize, BatchDecision> = HashMap::new();
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
        let decision = caps.get(2).unwrap().as_str().trim();
        let token = decision
            .split_once('|')
            .map(|(token, _)| token.trim())
            .unwrap_or(decision);
        if token.eq_ignore_ascii_case("SKIP") || ParsedAction::from_str(token).is_ok() {
            map.insert(
                idx,
                BatchDecision {
                    action: ParsedAction::from_str(token).ok(),
                    llm_response: format!("{}: {}", idx, decision),
                },
            );
        }
    }
    (1..=count)
        .map(|i| map.get(&i).cloned().unwrap_or_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_indexed_token_lines() {
        let parsed = parse_batch("1: ARCHIVE\n2: LABEL: Invoices\n3: SKIP\n", 3);
        assert_eq!(parsed[0].action, Some(ParsedAction::Archive));
        assert_eq!(
            parsed[1].action,
            Some(ParsedAction::Label("Invoices".into()))
        );
        assert_eq!(parsed[2].action, None);
    }

    #[test]
    fn missing_index_is_skip() {
        let parsed = parse_batch("1: TRASH\n3: STAR", 3);
        assert_eq!(parsed[0].action, Some(ParsedAction::Trash));
        assert_eq!(parsed[1].action, None);
        assert_eq!(parsed[2].action, Some(ParsedAction::Star));
    }

    #[test]
    fn garbled_line_is_ignored() {
        let parsed = parse_batch("1: ARCHIVE\nbanana\n2: SPAM", 2);
        assert_eq!(parsed[0].action, Some(ParsedAction::Archive));
        assert_eq!(parsed[1].action, Some(ParsedAction::Spam));
    }

    #[test]
    fn retains_each_email_explanation() {
        let parsed = parse_batch(
            "1: LABEL: Finance | The subject is an invoice from the bank.\n",
            1,
        );

        assert_eq!(
            parsed[0].action,
            Some(ParsedAction::Label("Finance".into()))
        );
        assert_eq!(
            parsed[0].llm_response,
            "1: LABEL: Finance | The subject is an invoice from the bank."
        );
    }
}
